use argon2::{password_hash::PasswordVerifier, Argon2, PasswordHash};
use axum::{
    extract::State,
    http::StatusCode,
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

pub async fn admin_get(
    State(aps): State<AppState>,
    // ConnectInfo(client_address): ConnectInfo<SocketAddr>,
    jar: CookieJar,
) -> impl IntoResponse {
    // Calculate the base64url-encoded sha256sum of the session cookie, if any.
    let user_session_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(
        URL_SAFE_NO_PAD
            .decode(jar.get("id").map_or("", |e| e.value()))
            .unwrap_or_default(),
    ));

    let session_row: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM admin_sessions WHERE session_id_sha256sum = ? LIMIT 1;")
            .bind(&user_session_sha256sum)
            .fetch_optional(&aps.db)
            .await
            .unwrap();

    if session_row.is_some() {
        #[derive(FromRow)]
        struct FileRow {
            efd_sha256sum: String,
            filesize: i64,
            upload_ts: String,
            expiry_ts: String,
            downloads: i64,
        }
        // Request info about all currently live files.
        let all_files: Vec<FileRow> = sqlx::query_as("SELECT efd_sha256sum, filesize, upload_ts, expiry_ts, downloads FROM uploaded_files WHERE expired = 0;")
            .fetch_all(&aps.db)
            .await
            .unwrap();

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
                let uts = DateTime::parse_from_rfc3339(&e.upload_ts).unwrap();
                let ets = DateTime::parse_from_rfc3339(&e.expiry_ts).unwrap();
                UploadedFile {
                    efd_sha256sum: e.efd_sha256sum,
                    formatted_filesize: format!("{:.2} MB", e.filesize as f64 / 1_000_000.0),
                    upload_ts_pretty: format!("{} ago", pretty_print_delta(now, uts)),
                    upload_ts: uts.format("(%c)").to_string(),
                    expiry_ts_pretty: pretty_print_delta(now, ets),
                    expiry_ts: ets.format("(%c)").to_string(),
                    downloads: e.downloads,
                }
            })
            .collect::<Vec<_>>();

        aps.tera.lock().unwrap().full_reload().unwrap();
        let mut context = Context::new();
        context.insert("files", &ufs);
        let h = aps
            .tera
            .lock()
            .unwrap()
            .render("admin_overview.html", &context)
            .unwrap();
        Html(String::from_utf8(minify(h.as_bytes(), &MINIFY_CFG)).unwrap())
    } else {
        aps.tera.lock().unwrap().full_reload().unwrap();
        let context = Context::new();
        let h = aps
            .tera
            .lock()
            .unwrap()
            .render("admin.html", &context)
            .unwrap();
        Html(String::from_utf8(minify(h.as_bytes(), &MINIFY_CFG)).unwrap())
    }
}

#[derive(Debug, Deserialize)]
pub struct AdminLogin {
    password: String,
    long_login: Option<String>,
}

#[axum::debug_handler]
pub async fn admin_post(
    State(aps): State<AppState>,
    // ConnectInfo(client_address): ConnectInfo<SocketAddr>,
    jar: CookieJar,
    Form(admin_login): Form<AdminLogin>,
) -> Result<(CookieJar, Redirect), StatusCode> {
    // TODO Obviously read this from a config, this is just for dev purposes.
    let admin_pw = PasswordHash::new("$argon2id$v=19$m=32768,t=2,p=1$GqrzTtRpoGeTSuq4$9rKEnqGUHRD1BkLq4IIa3CEFsuzsWwf646249huVPZk").unwrap();

    // Verify the provided password.
    Argon2::default()
        .verify_password(admin_login.password.as_bytes(), &admin_pw)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Create a new session.
    let session_id_bytes = thread_rng().gen::<[u8; 32]>();
    let session_id = URL_SAFE_NO_PAD.encode(&session_id_bytes);

    // Use the sha256-digest in the databse.
    let session_id_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(&session_id_bytes));

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

    // Calculate the RFC3339 timestamp for session expiry, either in one or 30 days.
    let expiry_ts = Utc::now()
        .checked_add_signed(TimeDelta::days(match admin_login.long_login {
            Some(_) => 30,
            None => 1,
        }))
        .unwrap()
        .to_rfc3339();

    sqlx::query("INSERT INTO admin_sessions (session_id_sha256sum, expiry_ts) VALUES (?, ?);")
        .bind(&session_id_sha256sum)
        .bind(&expiry_ts)
        .execute(&aps.db)
        .await
        .unwrap();

    Ok((jar.add(session_cookie), Redirect::to("/admin")))
}
