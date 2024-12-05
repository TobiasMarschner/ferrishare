//! Endpoint and utilities for automatic and manual file deletion

use axum::{extract::State, http::StatusCode, Json};
use axum_extra::extract::CookieJar;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::*;

#[derive(Debug, Deserialize)]
pub struct DeleteRequest {
    hash: String,
    admin: Option<String>,
}

/// Endpoint where clients can POST to delete files from the service before they expire.
pub async fn delete_endpoint(
    State(aps): State<AppState>,
    jar: CookieJar,
    Json(req): Json<DeleteRequest>,
) -> Result<StatusCode, AppError> {
    // Extract the two parameters.
    let efd_sha256sum = req.hash;
    let admin_key = req.admin;

    // Do not entertain hashes with invalid length.
    if efd_sha256sum.len() != 43 {
        return AppError::err(StatusCode::BAD_REQUEST, "invalid hash length");
    }

    // Query the databse for the entry.
    let row: Option<String> = sqlx::query_scalar(
        "SELECT admin_key_sha256sum FROM uploaded_files WHERE efd_sha256sum = ? LIMIT 1;",
    )
    .bind(&efd_sha256sum)
    .fetch_optional(&aps.db)
    .await?;

    // Return 404 if the file genuinely does not exist or has already expired.
    if row.is_none() {
        return AppError::err(StatusCode::NOT_FOUND, "file not found or expired");
    }

    // Guaranteed to work.
    let db_admin_key_sha256sum = row.ok_or_else(|| AppError::new500("illegal unwrap"))?;

    let mut authorized = false;

    // Compute the sha256-digest of the admin_key if it was provided.
    if let Some(admin_key) = admin_key {
        let admin_key_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(
            URL_SAFE_NO_PAD
                .decode(admin_key)
                .ok()
                .filter(|v| v.len() == 32)
                .unwrap_or_default(),
        ));

        // If the admin key matches, the request can go through.
        if admin_key_sha256sum == db_admin_key_sha256sum {
            authorized = true;
        }
    }

    // No matching admin_key? Check for session_id, then.
    // This is for the case where the deletion request is not made by the user who uploaded the
    // file, but by the site-wide administrator who is currently logged into the admin panel.
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
        // Use the cleanup method and bubble up any internal server errors.
        cleanup_file(&efd_sha256sum, &aps.db).await?;
        // Log the successful deletion.
        tracing::info!(efd_sha256sum, "manually deleted file");
        Ok(StatusCode::OK)
    } else {
        AppError::err(StatusCode::UNAUTHORIZED, "unauthorized")
    }
}

/// Remove a single file identified by its efd_sha256sum from the database and disk.
pub async fn cleanup_file(efd_sha256sum: &str, db: &SqlitePool) -> Result<(), anyhow::Error> {
    // First, remove the corresponding row form the DB.
    sqlx::query("DELETE FROM uploaded_files WHERE efd_sha256sum = ?;")
        .bind(efd_sha256sum)
        .execute(db)
        .await?;

    // Next, remove the actual file from disk.
    tokio::fs::remove_file(format!("{DATA_PATH}/uploaded_files/{}", efd_sha256sum)).await?;

    // If neither yielded an Error, return Ok.
    Ok(())
}
