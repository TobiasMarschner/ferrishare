use axum::{extract::State, http::StatusCode, Json};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::FromRow;

use crate::*;

#[derive(Debug, Deserialize)]
pub struct DeleteRequest {
    hash: String,
    admin: String,
}

pub async fn delete_endpoint(
    State(aps): State<AppState>,
    // ConnectInfo(client_address): ConnectInfo<SocketAddr>,
    Json(req): Json<DeleteRequest>,
) -> StatusCode {
    // Extract the two parameters.
    let efd_sha256sum = req.hash;
    let admin_key = req.admin;

    #[derive(FromRow)]
    struct FileRow {
        admin_key_sha256sum: String,
        expired: bool,
    }

    // Query the databse for the entry.
    let row: Option<FileRow> = sqlx::query_as("SELECT admin_key_sha256sum, expired FROM uploaded_files WHERE efd_sha256sum = ? LIMIT 1;")
        .bind(&efd_sha256sum)
        .fetch_optional(&aps.db)
        .await
        .unwrap();

    // Return 404 if the file genuinely does not exist or has already expired.
    if row.as_ref().map_or(true, |e| e.expired) {
        return StatusCode::NOT_FOUND;
    }

    // Guaranteed to work.
    let row = row.unwrap();

    // Compute the sha256-digest of the admin_key.
    let admin_key_sha256sum =
        URL_SAFE_NO_PAD.encode(Sha256::digest(URL_SAFE_NO_PAD.decode(admin_key).unwrap()));

    // If the hashes don't match, stop here.
    if admin_key_sha256sum != row.admin_key_sha256sum {
        return StatusCode::UNAUTHORIZED;
    }

    // Looks like the request is valid.
    // Update the expired-bool in the databse.
    sqlx::query("UPDATE uploaded_files SET expired = 1 WHERE efd_sha256sum = ?;")
        .bind(&efd_sha256sum)
        .execute(&aps.db)
        .await
        .unwrap();

    // TODO Actually delete the file, too.
    // TODO We'll probably want to refactor this since deletions involve the same steps but can
    // happen either as the result of a manual request like this, or as the result of timed
    // expiry.

    StatusCode::OK
}
