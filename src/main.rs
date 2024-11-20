use axum::{
    extract::ConnectInfo,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Router,
};
use itertools::Itertools;
use sqlx::{migrate::MigrateDatabase, FromRow, Sqlite, SqlitePool};
use std::{net::SocketAddr, sync::Arc};
use tera::Tera;
use tokio::sync::Mutex;
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, services::ServeDir};
use tracing::{Instrument, Level};

// Use 'pub use' here so that all the normal modules only have
// to import 'crate::*' instead of also having to import 'crate::error_handling::AppError'.
pub use error_handling::AppError;

mod admin;
mod auto_cleanup;
mod config;
mod delete;
mod download;
mod error_handling;
mod upload;

/// Global variables provided to every single request handler.
/// Contains pointers to the database-pool and HTML-templating-engine.
///
/// Implemented as a Cloneable struct containing two Arcs instead as copying around two pointers
/// should be cheaper than wrapping the whole struct in an Arc and suffering from two layers of
/// indirection on the database pool (as SqlitePool is itself essentially an Arc).
#[derive(Debug, Clone)]
pub struct AppState {
    tera: Arc<Mutex<Tera>>,
    db: SqlitePool,
}

/// Global definition of the HTML-minifier configuration.
///
/// CSS- and JS-minification are enabled, while some more aggressive
/// and non-compliant settings for HTML have been disabled.
pub const MINIFY_CFG: minify_html::Cfg = minify_html::Cfg {
    do_not_minify_doctype: true,
    ensure_spec_compliant_unquoted_attribute_values: true,
    keep_closing_tags: true,
    keep_html_and_head_opening_tags: true,
    keep_spaces_between_attributes: true,
    keep_comments: false,
    keep_input_type_text_attr: false,
    keep_ssi_comments: false,
    preserve_brace_template_syntax: false,
    preserve_chevron_percent_template_syntax: false,
    minify_css: true,
    minify_js: true,
    remove_bangs: false,
    remove_processing_instructions: false,
};

/// Path where all app-specific data will be stored.
///
/// This includes:
/// - The configuration at 'config.toml'
/// - The database at 'sqlite.db'
/// - All uploaded files in 'uploaded_files/'
const DATA_PATH: &str = "./data";

const DB_URL: &str = "sqlite://data/sqlite.db";

/// Custom middleware for tracing HTTP requests.
///
/// I am aware of tower_http::trace::TraceLayer but have opted not to use it.
/// Ultimately, this boils down to the client request IP + port.
/// http:Request does not contain information about the client IP+port making the request.
/// Instead, it has to be extracted using the ConnectInfo extractor provided by axum.
///
/// The middleware creates an "http_request" span wrapping the entire request and
/// fires off an event at the beginning which is then logged by the fmt Subscriber.
async fn custom_tracing(
    ConnectInfo(client): ConnectInfo<SocketAddr>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    // Extract all relevant info from the request.
    let path = request.uri().path();
    let query = request.uri().query().map(|v| {
        // Remove "admin=XXX" query parameter since the plaintext admin_key of a single file
        // is not supposed to be stored anywhere on the server, not even in the logs.
        v.split('&')
            .map(|p| {
                if p.starts_with("admin=") {
                    "admin=<REDACTED IN LOGS>"
                } else {
                    p
                }
            })
            .join("&")
    });
    // let version = request.version();
    let method = request.method();

    // Create the http_request span out of this info.
    let span = tracing::info_span!("http_request", %client, path, query, ?method);

    // Instrument the rest of the stack with this span.
    async move {
        // Actually process the request.
        let response = next.run(request).await;
        // Afterwards fire off an event so that the request + response StatusCode gets logged.
        tracing::info!(response.status = %response.status(), "processed request");
        response
    }
    .instrument(span)
    .await
}

#[tokio::main]
async fn main() {
    // First things first, create the DATA_PATH and its subdirectories.
    std::fs::create_dir_all(format!("{DATA_PATH}/uploaded_files")).expect(&format!(
        "Failed to recursively create directories: {DATA_PATH}/uploaded_files
These are required for the applications to store all of its data"
    ));

    config::setup_config();
    // Set up `tracing` (logging).
    // Use the default formatting subscriber provided by `tracing_subscriber`.
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    // Create the database if it doesn't already exist.
    if !Sqlite::database_exists(DB_URL).await.unwrap_or(false) {
        tracing::warn!("could not locate sqlite-db! creating a new one ...");
        match Sqlite::create_database(DB_URL).await {
            Ok(_) => {
                tracing::info!("successfully created new database");
            }
            Err(e) => {
                tracing::error!("failed to create database: {e}");
            }
        }
    }

    // Open the DB pool.
    let db = match SqlitePool::connect(DB_URL).await {
        Ok(db) => {
            tracing::info!("successfully opened database");
            db
        }
        Err(e) => {
            tracing::error!("failed to open database: {e}");
            return;
        }
    };

    // Initialize the templating engine.
    let tera = match Tera::new("templates/**/*.{html,js}") {
        Ok(t) => {
            tracing::info!("successfully loaded and compiled HTML and JS templates");
            t
        }
        Err(e) => {
            tracing::error!("failed to load and compile HTML and JS templates: {e}");
            return;
        }
    };
    // Wrap it in an Arc<Mutex<_>>, as required by AppState.
    let tera = Arc::new(Mutex::new(tera));

    // Perform migrations, if necesary.
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
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

    // Create the AppState out of database and template-engine.
    let aps = AppState { tera, db };

    // Start the background-task that regularly cleans up expired files and sessions.
    tokio::spawn(auto_cleanup::cleanup_cronjob(aps.clone()));

    // Define the app's routes.
    let app = Router::new()
        // HTML routes
        .route("/", get(upload::upload_page))
        .route("/file", get(download::download_page))
        .route("/admin", get(admin::admin_page))
        // API / non-HTML routes
        .route("/admin_login", post(admin::admin_login))
        .route("/admin_logout", post(admin::admin_logout))
        .route("/upload_endpoint", post(upload::upload_endpoint))
        .route("/download_endpoint", get(download::download_endpoint))
        .route("/delete_endpoint", post(delete::delete_endpoint))
        // Serve static assets from the 'static'-folder.
        .nest_service("/static", ServeDir::new("static"))
        // Enable response compression.
        .layer(ServiceBuilder::new().layer(CompressionLayer::new()))
        // Use our custom middleware for tracing.
        .layer(middleware::from_fn(custom_tracing))
        // Attach DB-pool and TERA-object as State.
        .with_state(aps)
        // Ensure client IPs and ports can be extracted.
        .into_make_service_with_connect_info::<SocketAddr>();

    // Bind to localhost for now.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8000")
        .await
        .unwrap();

    tracing::info!("listening on {}", listener.local_addr().unwrap());

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
    tracing::info!("received shutdown signal");
}

/// Returns true if the expiry_ts lies in the past, i.e. the resource has expired.
pub fn has_expired(expiry_ts: &str) -> Result<bool, AppError> {
    Ok(chrono::DateTime::parse_from_rfc3339(expiry_ts)?
        .signed_duration_since(chrono::Utc::now())
        .num_seconds()
        .is_negative())
}
