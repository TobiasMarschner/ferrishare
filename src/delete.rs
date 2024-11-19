use axum::{extract::State, http::StatusCode, Json};
use axum_extra::extract::CookieJar;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::FromRow;

use crate::*;

#[derive(Debug, Deserialize)]
pub struct DeleteRequest {
    hash: String,
    admin: Option<String>,
}

pub async fn delete_endpoint(
    State(aps): State<AppState>,
    // ConnectInfo(client_address): ConnectInfo<SocketAddr>,
    jar: CookieJar,
    Json(req): Json<DeleteRequest>,
) -> Result<StatusCode, AppError> {
    // Extract the two parameters.
    let efd_sha256sum = req.hash;
    let admin_key = req.admin;

    #[derive(FromRow)]
    struct FileRow {
        admin_key_sha256sum: String,
        expired: bool,
    }

    // Query the databse for the entry.
    let row: Option<FileRow> = sqlx::query_as(
        "SELECT admin_key_sha256sum, expired FROM uploaded_files WHERE efd_sha256sum = ? LIMIT 1;",
    )
    .bind(&efd_sha256sum)
    .fetch_optional(&aps.db)
    .await?;

    // Return 404 if the file genuinely does not exist or has already expired.
    if row.as_ref().map_or(true, |e| e.expired) {
        return AppError::err(StatusCode::NOT_FOUND, "file not found or expired");
    }

    // Guaranteed to work.
    let row = row.ok_or_else(|| AppError::new500("illegal unwrap"))?;

    let mut authorized = false;

    // Compute the sha256-digest of the admin_key if it was provided.
    if let Some(admin_key) = admin_key {
        let admin_key_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(
            URL_SAFE_NO_PAD.decode(admin_key).unwrap_or_default(),
        ));

        // If the admin key matches, the request can go through.
        if admin_key_sha256sum == row.admin_key_sha256sum {
            authorized = true;
        }
    }

    // No matching admin_key? Check for session_id, then.
    if !authorized {
        // Calculate the base64url-encoded sha256sum of the session cookie, if any.
        let user_session_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(
            URL_SAFE_NO_PAD
                .decode(jar.get("id").map_or("", |e| e.value()))
                .unwrap_or_default(),
        ));

        let session_row: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM admin_sessions WHERE session_id_sha256sum = ? LIMIT 1;",
        )
        .bind(&user_session_sha256sum)
        .fetch_optional(&aps.db)
        .await?;

        if session_row.is_some() {
            authorized = true;
        }
    }

    // Now delete the file if we're authroized.
    if authorized {
        // Update the expired-bool in the databse.
        sqlx::query("UPDATE uploaded_files SET expired = 1 WHERE efd_sha256sum = ?;")
            .bind(&efd_sha256sum)
            .execute(&aps.db)
            .await?;

        // TODO Actually delete the file, too.
        // TODO We'll probably want to refactor this since deletions involve the same steps but can
        // happen either as the result of a manual request like this, or as the result of timed
        // expiry.
        
        tracing::info!(efd_sha256sum, "deleted file");

        Ok(StatusCode::OK)
    } else {
        AppError::err(StatusCode::UNAUTHORIZED, "unauthorized")
    }
}
