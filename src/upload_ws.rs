//! WebSocket endpoint for uploading files in chunks
//!
//! Reverse proxies such as Cloudflare cap the size of a single request body.
//! Sending the encrypted file as a series of WebSocket messages avoids that limit,
//! and lets the server stream the file to disk instead of buffering it in memory.
//!
//! Protocol:
//! 1. The client sends a JSON text message with the upload's metadata ([UploadMetadata]).
//! 2. The server replies `{"ready":true}` once the upload is allowed.
//! 3. The client sends the encrypted filedata as binary messages, in order.
//! 4. Once `filesize` bytes have arrived the server replies with [UploadFileResponse].
//!
//! Any error is reported as `{"error":"..."}`, after which the server closes the socket.

use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::Response,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{prelude::*, rng};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::upload::{check_upload_allowed, register_upload};
use crate::*;

/// Largest binary message the server accepts. Clients should send smaller chunks.
const MAX_CHUNK_SIZE: usize = 16 * 1024 * 1024;

/// How long the server waits for the next message before giving up on an upload.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
struct UploadMetadata {
    /// base64url-encoded encrypted filename
    e_filename: String,
    /// base64url-encoded IV of the filedata
    iv_fd: String,
    /// base64url-encoded IV of the filename
    iv_fn: String,
    /// "hour", "day" or "week"
    duration: String,
    /// Size of the encrypted filedata in bytes
    filesize: u64,
}

/// Handler that upgrades the connection to a WebSocket and receives the upload.
pub async fn upload_ws_endpoint(
    State(aps): State<AppState>,
    ExtractIpPrefix(eip): ExtractIpPrefix,
    ws: WebSocketUpgrade,
) -> Response {
    ws.max_message_size(MAX_CHUNK_SIZE)
        .on_upgrade(move |socket| handle_socket(aps, eip, socket))
}

async fn handle_socket(aps: AppState, eip: IpPrefix, mut socket: WebSocket) {
    // Any given IpPrefix may only upload one file at a time, as with the POST endpoint.
    let result = if !aps.uploading.write().await.insert(eip) {
        AppError::err(
            StatusCode::TOO_MANY_REQUESTS,
            "you are already uploading a file, please wait",
        )
    } else {
        let result = receive_upload(&aps, &eip, &mut socket).await;
        if !aps.uploading.write().await.remove(&eip) {
            tracing::error!("tried to remove {eip} from aps.uploading after an upload, but it wasn't in the set");
        }
        result
    };

    let reply = match result {
        Ok(response) => serde_json::to_string(&response).unwrap_or_default(),
        Err(e) => {
            if e.status_code.is_server_error() {
                tracing::error!(status_code = e.status_code.to_string(), e.message);
            } else {
                tracing::warn!(status_code = e.status_code.to_string(), e.message);
            }
            serde_json::json!({ "error": format!("{}: {}", e.status_code, e.message) }).to_string()
        }
    };
    let _ = socket.send(Message::Text(reply.into())).await;
    let _ = socket.send(Message::Close(None)).await;
}

/// Wait for the next message from the client, with a timeout.
async fn next_message(socket: &mut WebSocket) -> Result<Message, AppError> {
    match tokio::time::timeout(IDLE_TIMEOUT, socket.recv()).await {
        Err(_) => AppError::err(StatusCode::REQUEST_TIMEOUT, "upload timed out"),
        Ok(None) => AppError::err(StatusCode::BAD_REQUEST, "connection closed during upload"),
        Ok(Some(Err(e))) => AppError::err(StatusCode::BAD_REQUEST, format!("websocket error: {e}")),
        Ok(Some(Ok(msg))) => Ok(msg),
    }
}

fn decode_iv(name: &str, value: &str) -> Result<[u8; 12], AppError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                format!("{name} is not exactly 12 bytes long"),
            )
        })
}

async fn receive_upload(
    aps: &AppState,
    eip: &IpPrefix,
    socket: &mut WebSocket,
) -> Result<upload::UploadFileResponse, AppError> {
    let metadata: UploadMetadata = match next_message(socket).await? {
        Message::Text(text) => serde_json::from_str(&text).map_err(|e| {
            AppError::new(StatusCode::BAD_REQUEST, format!("invalid upload metadata: {e}"))
        })?,
        _ => return AppError::err(StatusCode::BAD_REQUEST, "expected upload metadata"),
    };

    let e_filename = URL_SAFE_NO_PAD
        .decode(&metadata.e_filename)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "invalid encrypted filename"))?;
    if e_filename.len() > 8192 {
        return AppError::err(
            StatusCode::BAD_REQUEST,
            "encrypted filename is too large (larger than 8KiB)",
        );
    }
    let iv_fd = decode_iv("iv_fd", &metadata.iv_fd)?;
    let iv_fn = decode_iv("iv_fn", &metadata.iv_fn)?;
    let hour_duration = match metadata.duration.as_str() {
        "hour" => 1,
        "day" => 24,
        "week" => 24 * 7,
        _ => return AppError::err(StatusCode::BAD_REQUEST, "invalid duration parameter"),
    };
    if metadata.filesize > aps.conf.maximum_filesize {
        return AppError::err(StatusCode::BAD_REQUEST, "encrypted file is too large");
    }

    check_upload_allowed(aps, eip).await?;

    // Stream the file into a temporary name, and only move it into place once it's complete.
    let partial_path = format!(
        "{DATA_PATH}/uploaded_files/.partial-{}",
        URL_SAFE_NO_PAD.encode(rng().random::<[u8; 16]>())
    );
    let result = receive_filedata(socket, &partial_path, metadata.filesize).await;
    let efd_sha256sum = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = tokio::fs::remove_file(&partial_path).await;
            return Err(e);
        }
    };
    tokio::fs::rename(
        &partial_path,
        format!("{DATA_PATH}/uploaded_files/{efd_sha256sum}"),
    )
    .await
    .map_err(|e| AppError::new500(format!("failed to move uploaded file into place: {e}")))?;

    register_upload(
        aps,
        eip,
        efd_sha256sum,
        &e_filename,
        iv_fd,
        iv_fn,
        metadata.filesize as i64,
        hour_duration,
    )
    .await
}

/// Write the binary messages to `path` and return the base64url sha256sum of their contents.
async fn receive_filedata(
    socket: &mut WebSocket,
    path: &str,
    filesize: u64,
) -> Result<String, AppError> {
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| AppError::new500(format!("failed to create file on disk: {e}")))?;
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;

    socket
        .send(Message::Text(r#"{"ready":true}"#.into()))
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, format!("websocket error: {e}")))?;

    while received < filesize {
        let chunk = match next_message(socket).await? {
            Message::Binary(chunk) => chunk,
            Message::Ping(_) | Message::Pong(_) => continue,
            _ => return AppError::err(StatusCode::BAD_REQUEST, "expected file data"),
        };
        received += chunk.len() as u64;
        if received > filesize {
            return AppError::err(
                StatusCode::BAD_REQUEST,
                "received more data than announced",
            );
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| {
            AppError::new500(format!("failed to write encrypted filedata to disk: {e}"))
        })?;
    }

    file.flush()
        .await
        .map_err(|e| AppError::new500(format!("failed to write encrypted filedata to disk: {e}")))?;

    Ok(URL_SAFE_NO_PAD.encode(hasher.finalize()))
}
