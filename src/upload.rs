//! Page and endpoint for uploading new files to the service

use axum::{
    extract::{multipart::MultipartRejection, Multipart, Request, State},
    http::StatusCode,
    response::Html,
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{prelude::Utc, SubsecRound, TimeDelta};
use minify_html::minify;
use rand::prelude::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::*;

/// Handler that serves the page where users can upload new files.
pub async fn upload_page(State(aps): State<AppState>) -> Result<Html<String>, AppError> {
    let mut context = aps.default_context();
    // Check if the server has hit its quota limit and serve the appropriate template.
    let html = if maximum_quota_reached(&aps).await? {
        aps.tera.render("full_quota.html", &context)?
    } else {
        context.insert(
            "max_filesize",
            &pretty_print_bytes(aps.conf.maximum_filesize),
        );
        context.insert("raw_max_filesize", &aps.conf.maximum_filesize);
        aps.tera.render("upload.html", &context)?
    };
    let response_body = String::from_utf8(minify(html.as_bytes(), &MINIFY_CFG))?;
    Ok(Html(response_body))
}

/// Middleware that keeps track of which IpPrefixes are currently uploading in aps.uploading
///
/// Implemented as a middleware to ensure the IpPrefix is guaranteed to be removed from
/// aps.uploading regardless of whether the handler returns 2XX, 4XX or even 5XX.
pub async fn upload_endpoint_wrapper(
    State(aps): State<AppState>,
    ExtractIpPrefix(eip): ExtractIpPrefix,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    if !aps.uploading.write().await.insert(eip) {
        AppError::err(
            StatusCode::TOO_MANY_REQUESTS,
            "you are already uploading a file, please wait",
        )
    } else {
        // Handle the request.
        let response = next.run(request).await;
        // Remove the IpPrefix from the request.
        if !aps.uploading.write().await.remove(&eip) {
            tracing::error!("tried to remove {eip} from aps.uploading on successful upload, but it wasn't in the set");
        }
        Ok(response)
    }
}

/// Endpoint where clients can POST (i.e. upload) new files.
pub async fn upload_endpoint(
    State(aps): State<AppState>,
    ExtractIpPrefix(eip): ExtractIpPrefix,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<UploadFileResponse>), AppError> {
    // Handle bad multipart form data in here.
    // If something went wrong parsing it, blame the client.
    let mut multipart = multipart.map_err(|_| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "failed to parse form data; is your file too large?",
        )
    })?;

    // Find out how many files this user has already uploaded.
    let uploads_by_eip: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM uploaded_files WHERE upload_ip = ?;")
            .bind(eip.to_string())
            .fetch_one(&aps.db)
            .await?;

    // Check if the user has hit their upload limit.
    if uploads_by_eip as usize >= aps.conf.maximum_uploads_per_ip {
        return AppError::err(StatusCode::TOO_MANY_REQUESTS, "your computer has reached the file upload limit; delete old files or wait for them to expire");
    }

    // Check if the server has hit its quota limits.
    if maximum_quota_reached(&aps).await? {
        return AppError::err(
            StatusCode::INSUFFICIENT_STORAGE,
            "server has reached maximum storage capacity; please try again later",
        );
    }

    let mut e_filename: Option<Vec<u8>> = None;
    let mut e_filedata: Option<Vec<u8>> = None;
    let mut iv_fd: Option<[u8; 12]> = None;
    let mut iv_fn: Option<[u8; 12]> = None;
    let mut hour_duration: Option<i64> = None;

    while let Some(field) = multipart.next_field().await? {
        let field_name = field.name().map_or(String::new(), |e| e.to_string());
        let field_data = field.bytes().await.map_err(|_| {
            AppError::new(
                StatusCode::BAD_REQUEST,
                format!(
                    "failed to extract form data for field {field_name}; is your file too large?"
                ),
            )
        })?;

        match field_name.as_str() {
            "e_filename" => {
                if field_data.len() > 8192 {
                    return AppError::err(
                        StatusCode::BAD_REQUEST,
                        "encrypted filename is too large (larger than 8KiB)",
                    );
                }
                e_filename = Some(Vec::from(field_data));
            }
            "e_filedata" => {
                if field_data.len() > aps.conf.maximum_filesize {
                    return AppError::err(StatusCode::BAD_REQUEST, "encrypted file is too large");
                }
                e_filedata = Some(Vec::from(field_data));
            }
            "iv_fd" => {
                if field_data.len() != 12 {
                    return AppError::err(
                        StatusCode::BAD_REQUEST,
                        "iv_fd is not exactly 12 bytes long",
                    );
                }
                iv_fd = Some(Vec::from(field_data).try_into().unwrap());
            }
            "iv_fn" => {
                if field_data.len() != 12 {
                    return AppError::err(
                        StatusCode::BAD_REQUEST,
                        "iv_fn is not exactly 12 bytes long",
                    );
                }
                iv_fn = Some(Vec::from(field_data).try_into().unwrap());
            }
            "duration" => {
                let s = std::str::from_utf8(&field_data).map_err(|_| {
                    AppError::new(StatusCode::BAD_REQUEST, "invalid duration parameter")
                })?;
                hour_duration = match s {
                    "hour" => Some(1),
                    "day" => Some(24),
                    "week" => Some(24 * 7),
                    _ => None,
                };
            }
            _ => {
                return AppError::err(StatusCode::BAD_REQUEST, "illegal form field during upload");
            }
        }
    }

    let e_filename = e_filename
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "no encrypted filename provided"))?;
    let e_filedata = e_filedata
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "no encrypted filedata provided"))?;
    let iv_fd = iv_fd.ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "no iv_fd provided"))?;
    let iv_fn = iv_fn.ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "no iv_fn provided"))?;
    let hour_duration = hour_duration
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "no duration provided"))?;
    let filesize = e_filedata.len() as i64;
    let upload_ip = eip.to_string();

    // Compute the sha256sum of the encrypted data.
    // Likelihood of collision is ridiculously small, so we can ignore it here.
    // We'll use its base64url-encoding as the URL to identify the file.
    let efd_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(&e_filedata));

    // Generate a random admin password out of 256 bits of strong entropy.
    let admin_key_bytes = thread_rng().gen::<[u8; 32]>();
    let admin_key = URL_SAFE_NO_PAD.encode(admin_key_bytes);

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
    let admin_key_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(admin_key_bytes));

    // Grab the current time.
    let now = Utc::now().round_subsecs(0);

    // Generate the rfc3339 timestamps from this.
    let upload_ts = now.to_rfc3339();
    let expiry_ts = now
        .checked_add_signed(if aps.conf.demo_mode {
            // If demo mode is enabled, all expiry timestamps are set to 15 minutes.
            TimeDelta::minutes(15)
        } else {
            TimeDelta::hours(hour_duration)
        })
        .ok_or_else(|| AppError::new500("failed to apply duration to current timestamp"))?
        .to_rfc3339();

    // Store the file using asynchronous IO.
    tokio::fs::File::create(format!("{DATA_PATH}/uploaded_files/{efd_sha256sum}"))
        .await
        .map_err(|e| AppError::new500(format!("failed to create file on disk: {e}")))?
        .write_all(&e_filedata)
        .await
        .map_err(|e| {
            AppError::new500(format!("failed to write encrypted filedata to disk: {e}"))
        })?;

    // Then, add the row to the database.
    sqlx::query("INSERT INTO uploaded_files (efd_sha256sum, admin_key_sha256sum, e_filename, iv_fd, iv_fn, filesize, upload_ip, upload_ts, expiry_ts) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);")
        .bind(&efd_sha256sum)
        .bind(&admin_key_sha256sum)
        .bind(&e_filename)
        .bind(&iv_fd[..])
        .bind(&iv_fn[..])
        .bind(filesize)
        .bind(&upload_ip)
        .bind(&upload_ts)
        .bind(&expiry_ts)
        .execute(&aps.db)
        .await
        .map_err(|e| AppError::new500(format!("failed to insert row into database: {e}")))?;

    tracing::info!(
        efd_sha256sum,
        filesize,
        hour_duration,
        "succesfully created new file"
    );

    Ok((
        StatusCode::CREATED,
        Json(UploadFileResponse {
            efd_sha256sum,
            admin_key,
        }),
    ))
}

#[derive(Debug, Serialize)]
pub struct UploadFileResponse {
    efd_sha256sum: String,
    admin_key: String,
}

/// Helper function that checks whether the application has hit its global storage limit.
async fn maximum_quota_reached(aps: &AppState) -> Result<bool, AppError> {
    // Determine the total size of files uploaded so far.
    let total_quota: i64 = sqlx::query_scalar("SELECT SUM(filesize) FROM uploaded_files;")
        .fetch_one(&aps.db)
        .await?;

    // Check if we've hit the global limit.
    // In order to stay *strictly* underneath the limit, this function returns true
    // if the remaining space on disk is smaller than the biggest possible file.
    Ok(total_quota as usize
        >= aps
            .conf
            .maximum_quota
            .checked_sub(aps.conf.maximum_filesize)
            .unwrap_or_default())
}
