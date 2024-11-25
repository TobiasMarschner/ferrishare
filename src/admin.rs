use std::collections::HashMap;

use argon2::{password_hash::PasswordVerifier, Argon2, PasswordHash};
use axum::{
    extract::{Query, State},
    response::{Html, IntoResponse, Redirect},
    Form,
};
use axum_extra::extract::CookieJar;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{prelude::Utc, DateTime, TimeDelta};
use cookie::{time::Duration, Cookie};
use minify_html::minify;
use rand::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use tera::Context;

use crate::download::pretty_print_delta;
use crate::*;

pub async fn admin_page(
    Query(params): Query<HashMap<String, String>>,
    State(aps): State<AppState>,
    jar: CookieJar,
) -> Result<impl IntoResponse, AppError> {
    // Calculate the base64url-encoded sha256sum of the session cookie, if any.
    let user_session_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(
        URL_SAFE_NO_PAD
            .decode(jar.get("id").map_or("", |e| e.value()))
            .unwrap_or_default(),
    ));

    let session_expiry: Option<String> = sqlx::query_scalar(
        "SELECT expiry_ts FROM admin_sessions WHERE session_id_sha256sum = ? LIMIT 1;",
    )
    .bind(&user_session_sha256sum)
    .fetch_optional(&aps.db)
    .await?;

    // Only show the admin page if the session exists and has not yet expired.
    if !session_expiry.map_or(Ok(true), |v| has_expired(&v))? {
        #[derive(FromRow)]
        struct FileRow {
            efd_sha256sum: String,
            filesize: i64,
            upload_ts: String,
            expiry_ts: String,
            downloads: i64,
        }
        // Request info about all currently live files.
        let all_files: Vec<FileRow> = sqlx::query_as(
            "SELECT efd_sha256sum, filesize, upload_ts, expiry_ts, downloads FROM uploaded_files;",
        )
        .fetch_all(&aps.db)
        .await?;

        let used_quota: usize = all_files.iter().map(|e| e.filesize as usize).sum();

        #[derive(Debug, Serialize)]
        struct UploadedFile {
            efd_sha256sum: String,
            formatted_filesize: String,
            upload_ts_pretty: String,
            upload_ts: String,
            expiry_ts_pretty: String,
            expiry_ts: String,
            downloads: i64,
        }

        let now = Utc::now();

        let ufs = all_files
            .into_iter()
            .map(|e| {
                let uts = DateTime::parse_from_rfc3339(&e.upload_ts).ok();
                let ets = DateTime::parse_from_rfc3339(&e.expiry_ts).ok();
                UploadedFile {
                    efd_sha256sum: e.efd_sha256sum,
                    formatted_filesize: format!("{:.2} MB", e.filesize as f64 / 1_000_000.0),
                    upload_ts_pretty: if let Some(uts) = uts {
                        format!("{} ago", pretty_print_delta(now, uts))
                    } else {
                        "N/A".to_string()
                    },
                    upload_ts: if let Some(uts) = uts {
                        uts.format("(%c)").to_string()
                    } else {
                        "(invalid timestamp)".to_string()
                    },
                    expiry_ts_pretty: if let Some(ets) = ets {
                        pretty_print_delta(now, ets)
                    } else {
                        "N/A".to_string()
                    },
                    expiry_ts: if let Some(ets) = ets {
                        ets.format("(%c)").to_string()
                    } else {
                        "(invalid timestamp)".to_string()
                    },
                    downloads: e.downloads,
                }
            })
            .collect::<Vec<_>>();

        aps.tera.lock().await.full_reload()?;
        let mut context = Context::new();
        context.insert("files", &ufs);
        context.insert("full_file_count", &ufs.len());
        context.insert("maximum_quota", &pretty_print_bytes(aps.conf.maximum_quota));
        context.insert("used_quota", &pretty_print_bytes(used_quota));
        let h = aps
            .tera
            .lock()
            .await
            .render("admin_overview.html", &context)?;
        Ok(Html(String::from_utf8(minify(h.as_bytes(), &MINIFY_CFG))?))
    } else {
        // Check if this is a normal visit or a Redirect from a failed login-attempt.
        let failed_login = params.get("status").map_or(false, |e| e == "login_failed");

        aps.tera.lock().await.full_reload()?;
        let mut context = Context::new();
        context.insert("failed_login", &failed_login);
        let h = aps.tera.lock().await.render("admin_login.html", &context)?;
        Ok(Html(String::from_utf8(minify(h.as_bytes(), &MINIFY_CFG))?))
    }
}

#[derive(Debug, Deserialize)]
pub struct AdminLogin {
    password: String,
    long_login: Option<String>,
}

pub async fn admin_login(
    State(aps): State<AppState>,
    jar: CookieJar,
    Form(admin_login): Form<AdminLogin>,
) -> Result<(CookieJar, Redirect), AppError> {
    // Parse the config's PasswordHash
    let admin_pw = PasswordHash::new(&aps.conf.admin_password_hash).map_err(|e| {
        AppError::new500(format!(
            "the admin password hash in config.toml could not be parsed correctly: {e}"
        ))
    })?;

    // Verify the provided password.
    let password_result =
        Argon2::default().verify_password(admin_login.password.as_bytes(), &admin_pw);

    if password_result.is_err() {
        return Ok((jar, Redirect::to("/admin?status=login_failed")));
    }

    // Create a new session.
    let session_id_bytes = thread_rng().gen::<[u8; 32]>();
    let session_id = URL_SAFE_NO_PAD.encode(session_id_bytes);

    // Use the sha256-digest in the databse.
    let session_id_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(session_id_bytes));

    // Build the cookie for the session id.
    let mut session_cookie = Cookie::build(("id", session_id))
        .http_only(true)
        .secure(true)
        .same_site(cookie::SameSite::Strict);

    // If a long session is requested, it will be given 30 days validity.
    // Otherwise, it will be given 24 hours validity and
    // the cookie will be set to expire on closing the browser.

    if admin_login.long_login.is_some() {
        session_cookie = session_cookie.max_age(Duration::days(30));
    }

    // Session validity is either one or 30 days, depending on the login checkbox.
    let duration_days = admin_login.long_login.map_or(1, |_| 30);
    // Calculate the RFC3339 timestamp for session expiry, either in one or 30 days.
    let expiry_ts = Utc::now()
        .checked_add_signed(TimeDelta::days(duration_days))
        .ok_or_else(|| AppError::new500("failed to apply duration to current timestamp"))?
        .to_rfc3339();

    sqlx::query("INSERT INTO admin_sessions (session_id_sha256sum, expiry_ts) VALUES (?, ?);")
        .bind(&session_id_sha256sum)
        .bind(&expiry_ts)
        .execute(&aps.db)
        .await?;

    tracing::info!(session_id_sha256sum, duration_days, "new admin login");

    Ok((jar.add(session_cookie), Redirect::to("/admin")))
}

pub async fn admin_logout(
    State(aps): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, Redirect), AppError> {
    // Calculate the base64url-encoded sha256sum of the session cookie, if any.
    let user_session_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(
        URL_SAFE_NO_PAD
            .decode(jar.get("id").map_or("", |e| e.value()))
            .unwrap_or_default(),
    ));

    // Remove whatever rows exist with that sha256sum.
    let db_results = sqlx::query("DELETE FROM admin_sessions WHERE session_id_sha256sum = ?;")
        .bind(&user_session_sha256sum)
        .execute(&aps.db)
        .await?;

    tracing::info!(
        user_session_sha256sum,
        "logging out {0} admin session(s)",
        db_results.rows_affected()
    );

    Ok((jar.remove("id"), Redirect::to("/admin")))
}
