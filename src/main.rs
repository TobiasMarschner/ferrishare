use axum::{
    extract::{ConnectInfo, Multipart, State},
    response::{Html, IntoResponse},
    routing::{get, post},
    Router,
};
use chrono::{prelude::Utc, SubsecRound, TimeDelta};
use minify_html::minify;
use sqlx::{migrate::MigrateDatabase, FromRow, Row, Sqlite, SqlitePool};
use std::{
    fs::File,
    io::prelude::*,
    net::SocketAddr,
    sync::{LazyLock, Mutex},
};
use tera::{Context, Tera};
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, services::ServeDir};
use rand::prelude::*;
use base64::{engine::general_purpose::URL_SAFE, Engine as _};
use sha2::{Sha256, Digest};

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
        .route("/", get(root))
        .route("/admin", get(admin))
        .route("/download", get(download))
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

async fn root() -> impl IntoResponse {
    TERA.lock().unwrap().full_reload().unwrap();
    let context = Context::new();
    let h = TERA.lock().unwrap().render("index.html", &context).unwrap();
    Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap())
}

async fn download() -> impl IntoResponse {
    TERA.lock().unwrap().full_reload().unwrap();
    let context = Context::new();
    let h = TERA
        .lock()
        .unwrap()
        .render("download.html", &context)
        .unwrap();
    Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap())
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
    admin_key: String,
    e_filename: Vec<u8>,
    iv_fd: [u8; 12],
    iv_fn: [u8; 12],
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
) {
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

    // Grab all URL slugs and generate a new unique one.
    let all_slugs: Vec<String> = sqlx::query_scalar("SELECT url_slug FROM uploaded_files")
        .fetch_all(&db)
        .await
        .unwrap();

    // Compute the SHA-256 hash of the encrypted data.
    // Likelihood of collision is ridiculously small, so we can ignore it here.
    // It'll be used as the URL slug to access the file.
    let efd_sha256sum = URL_SAFE.encode(Sha256::digest(&e_filedata));
    // Admin keys don't have to be unique, just generate one.
    let admin_key = URL_SAFE.encode(thread_rng().gen::<[u8; 12]>());

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
    sqlx::query("INSERT INTO uploaded_files (url_slug, admin_key, e_filename, iv_fd, iv_fn, filesize, upload_ip, upload_ts, expiry_ts) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);")
        .bind(efd_sha256sum)
        .bind(admin_key)
        .bind(e_filename)
        .bind(&iv_fd[..])
        .bind(&iv_fn[..])
        .bind(filesize)
        .bind(upload_ip)
        .bind(upload_ts)
        .bind(expiry_ts)
        .execute(&db)
        .await
        .unwrap();
}
