use axum::{
    body::Body,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{prelude::Utc, DateTime, TimeZone};
use itertools::*;
use minify_html::minify;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use std::collections::HashMap;
use tera::Context;
use tokio_util::io::ReaderStream;

use crate::*;

pub async fn download_endpoint(
    Query(params): Query<HashMap<String, String>>,
    State(aps): State<AppState>,
    // ConnectInfo(client_address): ConnectInfo<SocketAddr>,
) -> (StatusCode, impl IntoResponse) {
    // Only the file parameter is permitted here.
    let hash = params.get("hash");

    // First, ensure the given parameters are correct.
    if params.get("hash").is_none() || params.len() != 1 {
        return (StatusCode::BAD_REQUEST, Body::empty());
    }

    // Guaranteed to work.
    let hash = hash.unwrap();

    // TODO: Think about what happens if the file is deleted / expires as it's being downloaded.

    // Next, query for the given file.
    // We only need to know whether
    // (1) the row exists
    // (2) the 'expired' value of the row
    // (3) the 'downloads' value, as we're going to increment that
    let row: Option<(i64, bool)> = sqlx::query_as(
        "SELECT downloads, expired FROM uploaded_files WHERE efd_sha256sum = ? LIMIT 1;",
    )
    .bind(&hash)
    .fetch_optional(&aps.db)
    .await
    .unwrap();

    // Return 404 if the file genuinely does not exist or has already expired.
    if row.as_ref().map_or(true, |e| e.1) {
        return (StatusCode::NOT_FOUND, Body::empty());
    }

    // Guaranteed to work.
    let row = row.unwrap();

    // Open the AsyncRead-stream for the file.
    let file = match tokio::fs::File::open(format!("data/{}", hash)).await {
        Ok(file) => file,
        Err(_) => {
            // A file being in the DB but not on disk should not be possible.
            return (StatusCode::INTERNAL_SERVER_ERROR, Body::empty());
        }
    };

    let body = Body::from_stream(ReaderStream::new(file));

    // Add to the download count.
    sqlx::query("UPDATE uploaded_files SET downloads = ? WHERE efd_sha256sum = ?;")
        .bind(&(row.0 + 1))
        .bind(&hash)
        .execute(&aps.db)
        .await
        .unwrap();

    (StatusCode::OK, body)
}

// Use a struct for the download page template parameters.
// This helps us not forget any required parameters.
#[derive(Debug, Serialize)]
struct DownloadPageContext<'a> {
    response_type: &'a str,
    error_head: &'a str,
    error_text: &'a str,
    e_filename: &'a str,
    iv_fd: &'a str,
    iv_fn: &'a str,
    filesize: &'a str,
    upload_ts: &'a str,
    upload_ts_pretty: &'a str,
    expiry_ts: &'a str,
    expiry_ts_pretty: &'a str,
    downloads: &'a str,
}

impl Default for DownloadPageContext<'_> {
    fn default() -> Self {
        DownloadPageContext {
            response_type: "",
            error_head: "",
            error_text: "",
            e_filename: "",
            iv_fd: "",
            iv_fn: "",
            filesize: "0",
            upload_ts: "",
            upload_ts_pretty: "",
            expiry_ts: "",
            expiry_ts_pretty: "",
            downloads: "0",
        }
    }
}

pub fn pretty_print_delta<Tz1: TimeZone, Tz2: TimeZone>(
    a: DateTime<Tz1>,
    b: DateTime<Tz2>,
) -> String {
    let time_delta = a.signed_duration_since(b);

    let values = vec![
        time_delta.num_weeks(),
        time_delta.num_days() % 7,
        time_delta.num_hours() % 24,
        time_delta.num_minutes() % 60,
    ];
    if values.iter().all(|v| *v == 0) {
        return "<1m".into();
    }
    let characters = vec!['w', 'd', 'h', 'm'];
    values
        .iter()
        .map(|v| v.abs())
        .zip(characters.iter())
        .filter(|(v, _)| *v > 0)
        .map(|(v, c)| format!("{v}{c}"))
        .join(" ")
}

pub async fn download_page(
    Query(params): Query<HashMap<String, String>>,
    State(aps): State<AppState>,
    // ConnectInfo(client_address): ConnectInfo<SocketAddr>,
) -> (StatusCode, Html<String>) {
    aps.tera.lock().unwrap().full_reload().unwrap();

    let hash = params.get("hash");
    let admin = params.get("admin");

    // Only allow legal combinations of parameters.
    match (hash, admin, params.len()) {
        (Some(_), Some(_), 2) => {}
        (Some(_), None, 1) => {}
        _ => {
            // Make it clear the parameters are invalid and return right away.
            let dpc = DownloadPageContext {
                response_type: "error",
                error_head: "Bad request",
                error_text: "Only \"hash\" and \"admin\" are valid query parameters. Are you supplying them?",
                ..Default::default()
            };

            let h = aps
                .tera
                .lock()
                .unwrap()
                .render("download.html", &Context::from_serialize(&dpc).unwrap())
                .unwrap();

            return (
                StatusCode::BAD_REQUEST,
                Html(String::from_utf8(minify(h.as_bytes(), &MINIFY_CFG)).unwrap()),
            );
        }
    }

    // Guaranteed to work thanks to the previous match.
    let hash = hash.unwrap();

    #[derive(FromRow)]
    struct FileRow {
        admin_key_sha256sum: String,
        e_filename: Vec<u8>,
        iv_fd: Vec<u8>,
        iv_fn: Vec<u8>,
        filesize: i64,
        upload_ts: String,
        expiry_ts: String,
        downloads: i64,
        expired: bool,
    }

    // Grab the row from the DB.
    let row: Option<FileRow> = sqlx::query_as("SELECT admin_key_sha256sum, e_filename, iv_fd, iv_fn, filesize, upload_ts, expiry_ts, downloads, expired FROM uploaded_files WHERE efd_sha256sum = ? LIMIT 1;")
        .bind(&hash)
        .fetch_optional(&aps.db)
        .await
        .unwrap();

    // Return 404 if the file genuinely does not exist or has already expired.
    if row.as_ref().map_or(true, |e| e.expired) {
        let dpc = DownloadPageContext {
            response_type: "error",
            error_head: "Not found",
            error_text: "The file does not exist or has since expired.",
            ..Default::default()
        };

        let h = aps
            .tera
            .lock()
            .unwrap()
            .render("download.html", &Context::from_serialize(&dpc).unwrap())
            .unwrap();

        return (
            StatusCode::NOT_FOUND,
            Html(String::from_utf8(minify(h.as_bytes(), &MINIFY_CFG)).unwrap()),
        );
    }

    // Guaranteed to work.
    let row = row.unwrap();

    // Extract several variables that we'll need in all cases.
    let filesize = row.filesize.to_string();
    let efn = format!("[{}]", row.e_filename.iter().join(", "));
    let iv_fd = format!("[{}]", row.iv_fd.iter().join(", "));
    let iv_fn = format!("[{}]", row.iv_fn.iter().join(", "));

    // Also extract and convert several more variables.
    // We only need them if the admin key is given,
    // but due to lifetime issues we're already converting them here.
    let downloads = row.downloads.to_string();

    // Timestamps
    let uts = DateTime::parse_from_rfc3339(&row.upload_ts).unwrap();
    let ets = DateTime::parse_from_rfc3339(&row.expiry_ts).unwrap();
    let now = Utc::now();

    let upload_ts = uts.format("(%c)").to_string();
    let upload_ts_pretty = format!("{} ago", pretty_print_delta(now, uts));
    let expiry_ts = ets.format("(%c)").to_string();
    let expiry_ts_pretty = pretty_print_delta(now, ets);

    let dpc: DownloadPageContext;

    // Now, branch depending on whether there's an admin key.
    if let Some(admin) = admin {
        // Perform three steps:
        // 1) Turn the base64url-encoded admin_key to binary.
        // 2) Calculate the sha256sum of that key in binary format.
        // 3) Reencode the digest to base64url.
        let admin_key_sha256sum =
            URL_SAFE_NO_PAD.encode(Sha256::digest(URL_SAFE_NO_PAD.decode(admin).unwrap()));

        // Now, check if the hashes match.
        if admin_key_sha256sum == row.admin_key_sha256sum {
            dpc = DownloadPageContext {
                response_type: "admin",
                e_filename: &efn,
                iv_fd: &iv_fd,
                iv_fn: &iv_fn,
                filesize: &filesize,
                upload_ts: &upload_ts,
                upload_ts_pretty: &upload_ts_pretty,
                expiry_ts: &expiry_ts,
                expiry_ts_pretty: &expiry_ts_pretty,
                downloads: &downloads,
                ..Default::default()
            };
        } else {
            dpc = DownloadPageContext {
                response_type: "file",
                error_head: "Invalid \"admin\" parameter",
                error_text: "The \"admin\" parameter does not match the database record. Displaying normal file download instead.",
                e_filename: &efn,
                iv_fd: &iv_fd,
                iv_fn: &iv_fn,
                filesize: &filesize,
                ..Default::default()
            };
        };
    } else {
        dpc = DownloadPageContext {
            response_type: "file",
            e_filename: &efn,
            iv_fd: &iv_fd,
            iv_fn: &iv_fn,
            filesize: &filesize,
            ..Default::default()
        };
    }

    // Use the DownloadPageContext to actually render the template.
    let h = aps
        .tera
        .lock()
        .unwrap()
        .render("download.html", &Context::from_serialize(&dpc).unwrap())
        .unwrap();

    // Minify and return.
    (
        StatusCode::OK,
        Html(String::from_utf8(minify(h.as_bytes(), &MINIFY_CFG)).unwrap()),
    )
}