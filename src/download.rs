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
) -> Result<(StatusCode, impl IntoResponse), AppError> {
    // Only the file parameter is permitted here.
    let hash = params.get("hash");

    // First, ensure the given parameters are correct.
    if hash.is_none() || params.len() != 1 {
        return AppError::err(
            StatusCode::BAD_REQUEST,
            "provide only the 'hash' query parameter",
        );
    }

    // Guaranteed to work.
    let hash = hash.ok_or_else(|| AppError::new500("illegal unwrap"))?;

    // Do not even entertain the notion of hashes with invalid length.
    if hash.len() != 43 {
        return AppError::err(StatusCode::BAD_REQUEST, "invalid hash length");
    }

    // TODO: Think about what happens if the file is deleted / expires as it's being downloaded.

    #[derive(Debug, FromRow)]
    struct FileRow {
        expiry_ts: String,
        downloads: i64,
    }

    // Next, query for the given file.
    // We only need to know
    // (1) whether the row exists
    // (2) the expiry timestamp of the row
    // (2) the 'downloads' value of that row, as it's going to be incremented
    let row: Option<FileRow> = sqlx::query_as(
        "SELECT expiry_ts, downloads FROM uploaded_files WHERE efd_sha256sum = ? LIMIT 1;",
    )
    .bind(hash)
    .fetch_optional(&aps.db)
    .await?;

    // Return 404 if the file genuinely does not exist or has already expired.
    if row
        .as_ref()
        .map_or(Ok(true), |v| has_expired(&v.expiry_ts))?
    {
        return AppError::err(StatusCode::NOT_FOUND, "file not found or expired");
    }

    // Guaranteed to work.
    let row = row.ok_or_else(|| AppError::new500("illegal unwrap"))?;

    // Open the AsyncRead-stream for the file.
    let file = match tokio::fs::File::open(format!("{DATA_PATH}/uploaded_files/{}", hash)).await {
        Ok(file) => file,
        Err(_) => {
            // A file being in the DB but not on disk should not be possible.
            return AppError::err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "file in database but not on disk",
            );
        }
    };

    let body = Body::from_stream(ReaderStream::new(file));

    // Add to the download count.
    sqlx::query("UPDATE uploaded_files SET downloads = ? WHERE efd_sha256sum = ?;")
        .bind(row.downloads + 1)
        .bind(hash)
        .execute(&aps.db)
        .await?;

    Ok((StatusCode::OK, body))
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
    filesize_pretty: &'a str,
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
            filesize_pretty: "0 Bytes",
            upload_ts: "",
            upload_ts_pretty: "",
            expiry_ts: "",
            expiry_ts_pretty: "",
            downloads: "0",
        }
    }
}

impl DownloadPageContext<'_> {
    /// Convert the DownloadPageContext to a Tera context.
    ///
    /// This means
    /// 1) serializing all the values in the DownloadPageContext
    /// 2) merging it with the global state's default context
    /// 3) *and* setting up variables that set elements' visibility-classes
    fn to_context(&self, aps: &AppState) -> Result<Context, anyhow::Error> {
        let mut context = tera::Context::from_serialize(self)?;
        context.extend(aps.default_context());
        // Make the error-box visible if we have any error text.
        context.insert(
            "error_vis",
            if self.error_head.is_empty() && self.error_text.is_empty() {
                "hidden"
            } else {
                "flex"
            },
        );
        // The file-box and the basic fields in it (filename and size)
        // have to be visible in both file and admin mode.
        context.insert(
            "file_vis",
            if self.response_type == "file" || self.response_type == "admin" {
                "flex"
            } else {
                "hidden"
            },
        );
        // For admin mode in particular there's a few more elements that need to be visible.
        context.insert(
            "admin_vis",
            if self.response_type == "admin" {
                "flex"
            } else {
                "hidden"
            },
        );
        Ok(context)
    }
}

pub fn pretty_print_delta<Tz1: TimeZone, Tz2: TimeZone>(
    a: DateTime<Tz1>,
    b: DateTime<Tz2>,
) -> String {
    let time_delta = a.signed_duration_since(b);

    let values = [
        time_delta.num_weeks(),
        time_delta.num_days() % 7,
        time_delta.num_hours() % 24,
        time_delta.num_minutes() % 60,
    ];
    if values.iter().all(|v| *v == 0) {
        return "<1m".into();
    }
    let characters = ['w', 'd', 'h', 'm'];
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
) -> Result<(StatusCode, Html<String>), AppError> {
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

            let h = aps.tera.render("download.html", &dpc.to_context(&aps)?)?;

            return Ok((
                StatusCode::BAD_REQUEST,
                Html(String::from_utf8(minify(h.as_bytes(), &MINIFY_CFG))?),
            ));
        }
    }

    // Guaranteed to work thanks to the previous match.
    let hash = hash.ok_or_else(|| AppError::new500("illegal unwrap"))?;

    // Do not even entertain the notion of hashes with invalid length.
    if hash.len() != 43 {
        let dpc = DownloadPageContext {
            response_type: "error",
            error_head: "Invalid hash length",
            error_text: "The hash parameter is not correctly formatted. Is your link correct?",
            ..Default::default()
        };

        let h = aps.tera.render("download.html", &dpc.to_context(&aps)?)?;

        return Ok((
            StatusCode::BAD_REQUEST,
            Html(String::from_utf8(minify(h.as_bytes(), &MINIFY_CFG))?),
        ));
    }

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
    }

    // Grab the row from the DB.
    let row: Option<FileRow> = sqlx::query_as("SELECT admin_key_sha256sum, e_filename, iv_fd, iv_fn, filesize, upload_ts, expiry_ts, downloads FROM uploaded_files WHERE efd_sha256sum = ? LIMIT 1;")
        .bind(hash)
        .fetch_optional(&aps.db)
        .await?;

    // Return 404 if the file genuinely does not exist or has already expired.
    if row
        .as_ref()
        .map_or(Ok(true), |v| has_expired(&v.expiry_ts))?
    {
        let dpc = DownloadPageContext {
            response_type: "error",
            error_head: "Not found",
            error_text: "The file does not exist or has since expired.",
            ..Default::default()
        };

        let h = aps.tera.render("download.html", &dpc.to_context(&aps)?)?;

        return Ok((
            StatusCode::NOT_FOUND,
            Html(String::from_utf8(minify(h.as_bytes(), &MINIFY_CFG))?),
        ));
    }

    // Guaranteed to work.
    let row = row.ok_or_else(|| AppError::new500("illegal unwrap"))?;

    // Extract several variables that we'll need in all cases.
    let filesize = row.filesize.to_string();
    let filesize_pretty = pretty_print_bytes(row.filesize as usize);
    let efn = format!("[{}]", row.e_filename.iter().join(", "));
    let iv_fd = format!("[{}]", row.iv_fd.iter().join(", "));
    let iv_fn = format!("[{}]", row.iv_fn.iter().join(", "));

    // Also extract and convert several more variables.
    // We only need them if the admin key is given,
    // but due to lifetime issues we're already converting them here.
    let downloads = row.downloads.to_string();

    // Timestamps
    let uts = DateTime::parse_from_rfc3339(&row.upload_ts)?;
    let ets = DateTime::parse_from_rfc3339(&row.expiry_ts)?;
    let now = Utc::now();

    let upload_ts = uts.format("(%c)").to_string();
    let upload_ts_pretty = format!("{} ago", pretty_print_delta(now, uts));
    let expiry_ts = ets.format("(%c)").to_string();
    let expiry_ts_pretty = pretty_print_delta(now, ets);

    let dpc: DownloadPageContext;

    // Now, branch depending on whether there's an admin key.
    if let Some(admin) = admin {
        // Perform the following steps:
        // 1) Turn the base64url-encoded admin_key to binary.
        // 2) Check that the binary key is exactly 32 bytes long.
        //    Yes, sha256 is preimage-resistant for any length of data,
        //    but we know this invariant must hold for legitimate keys,
        //    so we might as well check it here regardless.
        // 2) Calculate the sha256sum of that key in binary format.
        // 3) Reencode the digest to base64url.
        let admin_key_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(
            URL_SAFE_NO_PAD
                .decode(admin)
                .ok()
                .filter(|v| v.len() == 32)
                .unwrap_or_default(),
        ));

        // Now, check if the hashes match.
        if admin_key_sha256sum == row.admin_key_sha256sum {
            dpc = DownloadPageContext {
                response_type: "admin",
                e_filename: &efn,
                iv_fd: &iv_fd,
                iv_fn: &iv_fn,
                filesize: &filesize,
                filesize_pretty: &filesize_pretty,
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
                filesize_pretty: &filesize_pretty,
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
            filesize_pretty: &filesize_pretty,
            ..Default::default()
        };
    }

    // Use the DownloadPageContext to actually render the template.
    let h = aps.tera.render("download.html", &dpc.to_context(&aps)?)?;

    // Minify and return.
    Ok((
        StatusCode::OK,
        Html(String::from_utf8(minify(h.as_bytes(), &MINIFY_CFG))?),
    ))
}
