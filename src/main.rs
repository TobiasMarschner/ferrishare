use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, PasswordVerifier, SaltString},
    Argon2, PasswordHash,
};
use axum::{
    extract::{ConnectInfo, Multipart, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{prelude::Utc, SubsecRound, TimeDelta};
use itertools::*;
use minify_html::minify;
use rand::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{migrate::MigrateDatabase, FromRow, Sqlite, SqlitePool};
use std::{
    collections::HashMap,
    fs::File,
    io::prelude::*,
    net::SocketAddr,
    sync::{LazyLock, Mutex},
};
use tera::{Context, Tera};
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, services::ServeDir};

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
        .route("/admin", get(admin))
        .route("/admin_link", get(admin_link))
        .route("/admin_overview", get(admin_overview))
        .route("/upload_endpoint", post(upload_endpoint))
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
    expiry_ts: &'a str,
    views: &'a str,
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
            expiry_ts: "",
            views: "0",
            downloads: "0",
        }
    }
}

async fn download_page(
    Query(params): Query<HashMap<String, String>>,
    State(db): State<SqlitePool>,
    ConnectInfo(client_address): ConnectInfo<SocketAddr>,
) -> (StatusCode, Html<String>) {
    TERA.lock().unwrap().full_reload().unwrap();

    let hash = params.get("hash");
    let admin = params.get("admin");

    // Only allow legal combinations of parameters.
    match (hash, admin, params.len()) {
        (Some(h), Some(a), 2) => {}
        (Some(h), None, 1) => {}
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
    let row: Option<UploadFileRow> = sqlx::query_as("SELECT id, efd_sha256sum, admin_key_hash, e_filename, iv_fd, iv_fn, filesize, upload_ip, upload_ts, expiry_ts, views, downloads, expired FROM uploaded_files WHERE efd_sha256sum = ? LIMIT 1;")
        .bind(&hash)
        .fetch_optional(&db)
        .await
        .unwrap();

    if row.is_none() {
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

    // Also extract and convert views and downloads.
    // We only need them if the admin key is given,
    // but due to lifetime issues we're already converting them here.
    let views = row.views.to_string();
    let downloads = row.downloads.to_string();

    let dpc: DownloadPageContext;

    // Now, branch depending on whether there's an admin key.
    if let Some(admin) = admin {
        // Construct the hash-object out of the string stored in the db.
        let parsed_hash = PasswordHash::new(&row.admin_key_hash).unwrap();
        let password_check = Argon2::default().verify_password(admin.as_bytes(), &parsed_hash);

        if password_check.is_err() {
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
        } else {
            dpc = DownloadPageContext {
                response_type: "admin",
                e_filename: &efn,
                iv_fd: &iv_fd,
                iv_fn: &iv_fn,
                filesize: &filesize,
                upload_ts: &row.upload_ts,
                expiry_ts: &row.expiry_ts,
                views: &views,
                downloads: &downloads,
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

async fn admin() -> impl IntoResponse {
    TERA.lock().unwrap().full_reload().unwrap();
    let context = Context::new();
    let h = TERA.lock().unwrap().render("admin.html", &context).unwrap();
    Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap())
}

#[derive(Debug, FromRow, Clone)]
struct UploadFileRow {
    id: i64,
    efd_sha256sum: String,
    admin_key_hash: String,
    e_filename: Vec<u8>,
    iv_fd: Vec<u8>,
    iv_fn: Vec<u8>,
    filesize: i64,
    upload_ip: String,
    upload_ts: String,
    expiry_ts: String,
    views: i64,
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

    // Compute the SHA-256 hash of the encrypted data.
    // Likelihood of collision is ridiculously small, so we can ignore it here.
    // It'll be used as the URL slug to access the file.
    let efd_sha256sum = URL_SAFE_NO_PAD.encode(Sha256::digest(&e_filedata));

    // Generate a random admin password as well as a salt for it.
    let admin_key = URL_SAFE_NO_PAD.encode(thread_rng().gen::<[u8; 12]>());
    let admin_key_salt = SaltString::generate(&mut OsRng);

    let argon2 = Argon2::default();
    let admin_key_hash = argon2
        .hash_password(admin_key.as_bytes(), &admin_key_salt)
        .unwrap()
        .to_string();

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
    sqlx::query("INSERT INTO uploaded_files (efd_sha256sum, admin_key_hash, e_filename, iv_fd, iv_fn, filesize, upload_ip, upload_ts, expiry_ts) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);")
        .bind(&efd_sha256sum)
        .bind(&admin_key_hash)
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

async fn download_endpoint(
    State(db): State<SqlitePool>,
    ConnectInfo(client_address): ConnectInfo<SocketAddr>,
) {
}
