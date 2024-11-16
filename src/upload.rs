use axum::{
    extract::{ConnectInfo, Multipart, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{prelude::Utc, SubsecRound, TimeDelta};
use minify_html::minify;
use rand::prelude::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{fs::File, io::prelude::*, net::SocketAddr};
use tera::Context;

use crate::*;

pub async fn upload_page(State(aps): State<AppState>) -> impl IntoResponse {
    aps.tera.lock().unwrap().full_reload().unwrap();
    let context = Context::new();
    let h = aps
        .tera
        .lock()
        .unwrap()
        .render("index.html", &context)
        .unwrap();
    Html(String::from_utf8(minify(h.as_bytes(), &MINIFY_CFG)).unwrap())
}


pub async fn upload_endpoint(
    State(aps): State<AppState>,
    ConnectInfo(client_address): ConnectInfo<SocketAddr>,
    mut multipart: Multipart,
) -> (StatusCode, Json<UploadFileResponse>) {
    println!("endpoint reached");

    let mut e_filename: Option<Vec<u8>> = None;
    let mut e_filedata: Option<Vec<u8>> = None;
    let mut iv_fd: Option<[u8; 12]> = None;
    let mut iv_fn: Option<[u8; 12]> = None;
    let mut hour_duration: Option<i64> = None;

    while let Some(field) = multipart.next_field().await.unwrap() {
        let field_name = field.name().unwrap().to_string();
        let field_data = field.bytes().await.unwrap();

        match field_name.as_str() {
            "e_filename" => {
                if field_data.len() > 8192 {
                    todo!("filename over 8KiB (somehow)");
                }
                e_filename = Some(Vec::from(field_data));
            }
            "e_filedata" => {
                if field_data.len() > 10485760 {
                    todo!("file bigger than 10MiB");
                }
                e_filedata = Some(Vec::from(field_data));
            }
            "iv_fd" => {
                if field_data.len() != 12 {
                    todo!("iv_fd is not exactly 12 bytes");
                }
                iv_fd = Some(Vec::from(field_data).try_into().unwrap());
            }
            "iv_fn" => {
                if field_data.len() != 12 {
                    todo!("iv_fn is not exactly 12 bytes");
                }
                iv_fn = Some(Vec::from(field_data).try_into().unwrap());
            }
            "duration" => {
                let s = std::str::from_utf8(&field_data).unwrap();
                hour_duration = match s {
                    "hour" => Some(1),
                    "day" => Some(24),
                    "week" => Some(24 * 7),
                    _ => None,
                };
            }
            _ => {
                todo!("illegal form field");
            }
        }
    }

    let e_filename = e_filename.unwrap();
    let e_filedata = e_filedata.unwrap();
    let iv_fd = iv_fd.unwrap();
    let iv_fn = iv_fn.unwrap();
    let hour_duration = hour_duration.unwrap();
    let filesize = e_filedata.len() as i64;
    let upload_ip = client_address.ip().to_string();

    // Compute the sha256sum of the encrypted data.
    // Likelihood of collision is ridiculously small, so we can ignore it here.
    // We'll use its base64url-encoding as the URL to identify the file.
    let efd_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(&e_filedata));

    // Generate a random admin password out of 256 bits of strong entropy.
    let admin_key_bytes = thread_rng().gen::<[u8; 32]>();
    let admin_key = URL_SAFE_NO_PAD.encode(&admin_key_bytes);

    // Also generate a hash of this password using sha256 for storage in the databse.
    //
    // NOTE: Use of sha256 instead of a password-hashing algorithm like argon2id is intentional.
    // Password-hashing algorithms help secure passwords that:
    // 1) may have little entropy to begin with (mitigated by increasing the algorithm's parameters,
    //    such as iteration count and memory footprint)
    // 2) may be used more than once by different users (mitigated by salting)
    // 3) may leak if the db gets hacked (mitigated since hashing is a one-way operation)
    //
    // Threat (1) does not apply since the passwords are generated with 256 bits of entropy.
    // Threat (2) does not apply since the password is randomly generated.
    //
    // This means only the third threat has to be considered.
    // For that purpose, a single iteration of sha256 is wholly sufficient.
    //
    // In practice, this choice helps speed up requests
    // as a single sha256-digest can be computed very quickly.
    let admin_key_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(&admin_key_bytes));

    // TODO Proxied IPs. You'll likely run this behind a reverse-proxy.
    // You need to be able to set up trusted proxy IPs and extract X-Forwarded-For instead.
    // Grab the current time.
    let now = Utc::now().round_subsecs(0);

    // Generate the rfc3339 timestamps from this.
    let upload_ts = now.to_rfc3339();
    let expiry_ts = now
        .checked_add_signed(TimeDelta::hours(hour_duration))
        .unwrap()
        .to_rfc3339();

    // First, store the file using std::io.
    let mut efile = File::create(format!("data/{efd_sha256sum}")).unwrap();
    efile.write_all(&e_filedata).unwrap();
    drop(efile);

    // TODO Calculate entropy of the file.
    // TODO Also check for magic number.
    // We need to guard against someone uploading unencrypted data directly to the server.
    // While not perfect, those techniques can help us catch the worst offenders.

    // Then, add the row to the database.
    sqlx::query("INSERT INTO uploaded_files (efd_sha256sum, admin_key_sha256sum, e_filename, iv_fd, iv_fn, filesize, upload_ip, upload_ts, expiry_ts) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);")
        .bind(&efd_sha256sum)
        .bind(&admin_key_sha256sum)
        .bind(&e_filename)
        .bind(&iv_fd[..])
        .bind(&iv_fn[..])
        .bind(&filesize)
        .bind(&upload_ip)
        .bind(&upload_ts)
        .bind(&expiry_ts)
        .execute(&aps.db)
        .await
        .unwrap();

    (
        StatusCode::CREATED,
        Json(UploadFileResponse {
            efd_sha256sum,
            admin_key,
        }),
    )
}

#[derive(Debug, Serialize)]
pub struct UploadFileResponse {
    efd_sha256sum: String,
    admin_key: String,
}
