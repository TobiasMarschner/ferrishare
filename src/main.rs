use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, PasswordVerifier, SaltString},
    Argon2, PasswordHash,
};
use axum::{
    body::Body,
    extract::{ConnectInfo, Multipart, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
    Form, Json, Router,
};
use axum_extra::extract::CookieJar;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{prelude::Utc, DateTime, SubsecRound, TimeDelta, TimeZone};
use cookie::{time::Duration, Cookie};
use itertools::*;
use minify_html::minify;
use rand::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{migrate::MigrateDatabase, FromRow, Sqlite, SqlitePool};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::prelude::*,
    net::SocketAddr,
    sync::{LazyLock, Mutex},
};
use tera::{Context, Tera};
use tokio_util::io::ReaderStream;
use tower::ServiceBuilder;
use tower_http::{
    compression::CompressionLayer,
    services::{fs::AsyncReadBody, ServeDir},
};

static HTML_MINIFY_CFG: LazyLock<minify_html::Cfg> = LazyLock::new(|| {
    let mut cfg = minify_html::Cfg::spec_compliant();
    // Keep things compliant, we don't need to crunc *that much*.
    cfg.keep_closing_tags = true;
    cfg.keep_html_and_head_opening_tags = true;
    // Very useful, minify all the CSS here, too.
    cfg.minify_css = true;
    cfg.minify_js = true;
    cfg
});

pub static TERA: LazyLock<Mutex<Tera>> = LazyLock::new(|| {
    let tera = match Tera::new("templates/**/*.{html,js}") {
        Ok(t) => t,
        Err(e) => {
            println!("Parsing error(s): {}", e);
            panic!();
        }
    };
    Mutex::new(tera)
});

const DB_URL: &str = "sqlite://sqlite.db";

#[tokio::main]
async fn main() {
    // Create the database if it doesn't already exist.
    if !Sqlite::database_exists(DB_URL).await.unwrap_or(false) {
        println!("Creating databse {DB_URL}");
        match Sqlite::create_database(DB_URL).await {
            Ok(_) => println!("Create db success"),
            Err(e) => panic!("creating db failed: {e}"),
        }
    }

    // Open the DB pool.
    let db = SqlitePool::connect(DB_URL).await.unwrap();

    // Perform migrations, if necesary.
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let migrations = std::path::Path::new(&crate_dir).join("./migrations");

    let migration_results = sqlx::migrate::Migrator::new(migrations)
        .await
        .unwrap()
        .run(&db)
        .await;

    match migration_results {
        Ok(_) => println!("migration success"),
        Err(e) => {
            panic!("error: {e}");
        }
    }

    println!("migration: {:?}", migration_results);

    // let result = sqlx::query("INSERT INTO uploaded_files (id, url_slug, admin_key, e_filename, iv_fd, iv_fn, filesize, upload_ip, upload_timestamp, expiry_timestamp) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?);")
    //     .bind(23)
    //     .bind("coolurl")
    //     .bind("adminkey")
    //     .bind("EXXX")
    //     .bind("IVFD")
    //     .bind("IVFN")
    //     .bind(0)
    //     .bind("127.0.0.1")
    //     .bind("today")
    //     .bind("tomorrow")
    //     .execute(&db)
    //     .await
    //     .unwrap();

    // println!("insert result: {:?}", result);

    // Define the app's routes.
    let app = Router::new()
        // Main routes
        .route("/", get(upload_page))
        .route("/file", get(download_page))
        .route("/admin", get(admin_get))
        .route("/admin", post(admin_post))
        .route("/admin_link", get(admin_link))
        .route("/admin_overview", get(admin_overview))
        .route("/upload_endpoint", post(upload_endpoint))
        .route("/download_endpoint", get(download_endpoint))
        .route("/delete_endpoint", post(delete_endpoint))
        // Serve static assets from the 'static'-folder.
        .nest_service("/static", ServeDir::new("static"))
        // Enable response compression.
        .layer(ServiceBuilder::new().layer(CompressionLayer::new()))
        .with_state(db)
        .into_make_service_with_connect_info::<SocketAddr>();

    // Bind to localhost for now.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8000")
        .await
        .unwrap();

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_handler())
        .await
        .unwrap();
}

async fn shutdown_handler() {
    // Wait for the CTRL+C signal
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C handler");

    // Received one? Print that, then hyper will shut down the server.
    println!("Received shutdown signal ...");
}

#[derive(Debug, Deserialize)]
struct DeleteRequest {
    hash: String,
    admin: String,
}

async fn delete_endpoint(
    State(db): State<SqlitePool>,
    ConnectInfo(client_address): ConnectInfo<SocketAddr>,
    Json(req): Json<DeleteRequest>,
) -> StatusCode {
    // Extract the two parameters.
    let efd_sha256sum = req.hash;
    let admin_key = req.admin;

    // Query the databse for the entry.
    let row: Option<UploadFileRow> = sqlx::query_as("SELECT id, efd_sha256sum, admin_key_sha256sum, e_filename, iv_fd, iv_fn, filesize, upload_ip, upload_ts, expiry_ts, downloads, expired FROM uploaded_files WHERE efd_sha256sum = ? LIMIT 1;")
        .bind(&efd_sha256sum)
        .fetch_optional(&db)
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
        .execute(&db)
        .await
        .unwrap();

    // TODO Actually delete the file, too.
    // TODO We'll probably want to refactor this since deletions involve the same steps but can
    // happen either as the result of a manual request like this, or as the result of timed
    // expiry.

    StatusCode::OK
}

async fn download_endpoint(
    Query(params): Query<HashMap<String, String>>,
    State(db): State<SqlitePool>,
    ConnectInfo(client_address): ConnectInfo<SocketAddr>,
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
    let row: Option<UploadFileRow> = sqlx::query_as("SELECT id, efd_sha256sum, admin_key_sha256sum, e_filename, iv_fd, iv_fn, filesize, upload_ip, upload_ts, expiry_ts, downloads, expired FROM uploaded_files WHERE efd_sha256sum = ? LIMIT 1;")
        .bind(&hash)
        .fetch_optional(&db)
        .await
        .unwrap();

    // Return 404 if the file genuinely does not exist or has already expired.
    if row.as_ref().map_or(true, |e| e.expired) {
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
        .bind(&(row.downloads + 1))
        .bind(&hash)
        .execute(&db)
        .await
        .unwrap();

    (StatusCode::OK, body)
}

async fn upload_page() -> impl IntoResponse {
    TERA.lock().unwrap().full_reload().unwrap();
    let context = Context::new();
    let h = TERA.lock().unwrap().render("index.html", &context).unwrap();
    Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap())
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

fn pretty_print_delta<Tz1: TimeZone, Tz2: TimeZone>(a: DateTime<Tz1>, b: DateTime<Tz2>) -> String {
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

async fn download_page(
    Query(params): Query<HashMap<String, String>>,
    State(db): State<SqlitePool>,
    // ConnectInfo(client_address): ConnectInfo<SocketAddr>,
) -> (StatusCode, Html<String>) {
    TERA.lock().unwrap().full_reload().unwrap();

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

            let h = TERA
                .lock()
                .unwrap()
                .render("download.html", &Context::from_serialize(&dpc).unwrap())
                .unwrap();

            return (
                StatusCode::BAD_REQUEST,
                Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap()),
            );
        }
    }

    // Guaranteed to work thanks to the previous match.
    let hash = hash.unwrap();

    // Grab the row from the DB.
    let row: Option<UploadFileRow> = sqlx::query_as("SELECT id, efd_sha256sum, admin_key_sha256sum, e_filename, iv_fd, iv_fn, filesize, upload_ip, upload_ts, expiry_ts, downloads, expired FROM uploaded_files WHERE efd_sha256sum = ? LIMIT 1;")
        .bind(&hash)
        .fetch_optional(&db)
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

        let h = TERA
            .lock()
            .unwrap()
            .render("download.html", &Context::from_serialize(&dpc).unwrap())
            .unwrap();

        return (
            StatusCode::NOT_FOUND,
            Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap()),
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
    let h = TERA
        .lock()
        .unwrap()
        .render("download.html", &Context::from_serialize(&dpc).unwrap())
        .unwrap();

    // Minify and return.
    (
        StatusCode::OK,
        Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap()),
    )
}

async fn admin_link() -> impl IntoResponse {
    TERA.lock().unwrap().full_reload().unwrap();
    let context = Context::new();
    let h = TERA
        .lock()
        .unwrap()
        .render("admin_link.html", &context)
        .unwrap();
    Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap())
}

async fn admin_overview() -> impl IntoResponse {
    TERA.lock().unwrap().full_reload().unwrap();
    let context = Context::new();
    let h = TERA
        .lock()
        .unwrap()
        .render("admin_overview.html", &context)
        .unwrap();
    Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap())
}

#[derive(Debug, FromRow, Deserialize)]
struct AdminSession {
    session_id_sha256sum: String,
    expiry_ts: String,
}

async fn admin_get(
    State(db): State<SqlitePool>,
    ConnectInfo(client_address): ConnectInfo<SocketAddr>,
    jar: CookieJar,
) -> impl IntoResponse {
    // Calculate the base64url-encoded sha256sum of the session cookie, if any.
    let user_session_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(
        URL_SAFE_NO_PAD
            .decode(jar.get("id").map_or("", |e| e.value()))
            .unwrap_or_default(),
    ));

    let session_row: Option<AdminSession> = sqlx::query_as("SELECT session_id_sha256sum, expiry_ts FROM admin_sessions WHERE session_id_sha256sum = ? LIMIT 1;").bind(&user_session_sha256sum)
        .fetch_optional(&db)
        .await
        .unwrap();

    if session_row.is_some() {
        // Request info about all currently live files.
        let all_files: Vec<UploadFileRow> = sqlx::query_as("SELECT id, efd_sha256sum, admin_key_sha256sum, e_filename, iv_fd, iv_fn, filesize, upload_ip, upload_ts, expiry_ts, downloads, expired FROM uploaded_files WHERE expired = 0;")
            .fetch_all(&db)
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

        TERA.lock().unwrap().full_reload().unwrap();
        let mut context = Context::new();
        context.insert("files", &ufs);
        let h = TERA
            .lock()
            .unwrap()
            .render("admin_overview.html", &context)
            .unwrap();
        Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap())
    } else {
        TERA.lock().unwrap().full_reload().unwrap();
        let context = Context::new();
        let h = TERA.lock().unwrap().render("admin.html", &context).unwrap();
        Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap())
    }
}

#[derive(Debug, Deserialize)]
struct AdminLogin {
    password: String,
    long_login: Option<String>,
}

#[axum::debug_handler]
async fn admin_post(
    State(db): State<SqlitePool>,
    ConnectInfo(client_address): ConnectInfo<SocketAddr>,
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
        .execute(&db)
        .await
        .unwrap();

    Ok((jar.add(session_cookie), Redirect::to("/admin")))
}

#[derive(Debug, FromRow, Clone)]
struct UploadFileRow {
    id: i64,
    efd_sha256sum: String,
    admin_key_sha256sum: String,
    e_filename: Vec<u8>,
    iv_fd: Vec<u8>,
    iv_fn: Vec<u8>,
    filesize: i64,
    upload_ip: String,
    upload_ts: String,
    expiry_ts: String,
    downloads: i64,
    expired: bool,
}

async fn upload_endpoint(
    State(db): State<SqlitePool>,
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
        .execute(&db)
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
struct UploadFileResponse {
    efd_sha256sum: String,
    admin_key: String,
}
