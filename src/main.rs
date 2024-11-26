use axum::{
    extract::{ConnectInfo, DefaultBodyLimit},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Router,
};
use clap::Parser;
use itertools::Itertools;
use sqlx::{migrate::MigrateDatabase, FromRow, Sqlite, SqlitePool};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    process::ExitCode,
    sync::Arc,
    time::Duration,
};
use tera::Tera;
use tokio::sync::{Mutex, RwLock};
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, services::ServeDir, timeout::TimeoutLayer};
use tracing::Instrument;

// Use 'pub use' here so that all the normal modules only have
// to import 'crate::*' instead of also having to import 'crate::error_handling::AppError'.
pub use config::AppConfiguration;
pub use error_handling::AppError;
pub use ip_prefix::{ExtractIpPrefix, IpPrefix};

mod admin;
mod auto_cleanup;
mod config;
mod delete;
mod download;
mod error_handling;
mod ip_prefix;
mod upload;

/// Global variables provided to every single request handler.
/// Contains pointers to the database-pool and HTML-templating-engine.
///
/// Implemented as a Cloneable struct containing two Arcs instead as copying around two pointers
/// should be cheaper than wrapping the whole struct in an Arc and suffering from two layers of
/// indirection on the database pool (as SqlitePool is itself essentially an Arc).
#[derive(Debug, Clone)]
pub struct AppState {
    /// global HTML/JS templating engine
    tera: Arc<Mutex<Tera>>,
    /// sqlite database
    ///
    /// Is internally implemented as an Arc, so no need to wrap it here.
    db: SqlitePool,
    /// immutable global configuration for the application
    conf: Arc<AppConfiguration>,
    /// table of request counts for rate limiting
    rate_limiter: Arc<RwLock<HashMap<IpPrefix, u64>>>,
    /// list of IpPrefixes who are currently uploading a file
    ///
    /// Any given IP is only allowed to stream one file at a time.
    /// Otherwise, a malicious client could start hundreds of uploads
    /// simultaneously and bypass quota restrictions.
    uploading: Arc<RwLock<HashSet<IpPrefix>>>,
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

/// TODO app description
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Run the interactive setup to create the 'config.toml'. (required before first launch)
    ///
    /// The reason app configuration is performed interactively like this instead of just providing
    /// an annotated config-file really boils down to the admin password. It needs to be stored as
    /// an argon2id-hash and manually creating one (e.g. with the 'argon2' CLI) is quite annoying.
    #[arg(long)]
    init: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    // First things first, create the DATA_PATH and its subdirectories.
    std::fs::create_dir_all(format!("{DATA_PATH}/uploaded_files")).unwrap_or_else(|_| {
        panic!(
            "Failed to recursively create directories: {DATA_PATH}/uploaded_files
These are required for the applications to store all of its data"
        )
    });

    // Parse cmd-line arguments and check whether we're (re-)creating the config.toml.
    if Args::parse().init {
        // Set up config and exit immediately.
        match config::setup_config() {
            Ok(_) => {
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                panic!("failed to create config: {e}");
            }
        }
    }

    // Try to open and parse the configuration.
    let config_string = match std::fs::read_to_string(format!("{DATA_PATH}/config.toml")) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Failed to open configuration file at {DATA_PATH}/config.toml: {e}");

            eprintln!(
                "\nIf you haven't already, configure the app by running it with the '--init' flag:"
            );
            eprintln!("  docker compose run -it [TODOTODO] --init     (for Docker Compose)");
            eprintln!("  cargo run -- --init                          (for cargo)");

            eprintln!("\nExiting!");
            return ExitCode::FAILURE;
        }
    };

    let app_config: AppConfiguration = match toml::from_str(&config_string) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Failed to parse configuration file at {DATA_PATH}/config.toml: {e}");

            eprintln!("\nIf your config file is causing trouble, consider regenerating it by running the app with the '--init' flag:");
            eprintln!("  docker compose run -it [TODOTODO] --init     (for Docker Compose)");
            eprintln!("  cargo run -- --init                          (for cargo)");

            eprintln!("\nExiting!");
            return ExitCode::FAILURE;
        }
    };

    // Set up `tracing` (logging).
    // Use the default formatting subscriber provided by `tracing_subscriber`.
    // The log level is provided by the configuration.
    tracing_subscriber::fmt()
        .with_max_level(app_config.translate_log_level())
        // .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
        .init();

    tracing::info!("read config from {DATA_PATH}/config.toml");

    // Create the database if it doesn't already exist.
    if !Sqlite::database_exists(DB_URL).await.unwrap_or(false) {
        tracing::warn!("could not locate sqlite-db! creating a new one ...");
        match Sqlite::create_database(DB_URL).await {
            Ok(_) => {
                tracing::info!("successfully created new database");
            }
            Err(e) => {
                tracing::error!("failed to create database: {e}");
                return ExitCode::FAILURE;
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
            return ExitCode::FAILURE;
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
            return ExitCode::FAILURE;
        }
    };
    // Wrap it in an Arc<Mutex<_>>, as required by AppState.
    let tera = Arc::new(Mutex::new(tera));

    // Perform database migrations (create all required tables).
    // Note that the migrate!-macro includes these in the binary at compile time.
    match sqlx::migrate!("./migrations").run(&db).await {
        Ok(_) => {
            tracing::info!("database migrations successful");
        }
        Err(e) => {
            tracing::error!("failed to perform databse migrations: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Create the AppState out of database and template-engine.
    let aps = AppState {
        tera,
        db,
        conf: Arc::new(app_config),
        rate_limiter: Arc::new(RwLock::new(HashMap::new())),
        uploading: Arc::new(RwLock::new(HashSet::new())),
    };
    // Keep a copy of the interface, we'll need it after the AppState has already been moved.
    let interface = aps.conf.interface.clone();

    // Start the background-task that regularly cleans up expired files and sessions.
    tokio::spawn(auto_cleanup::cleanup_cronjob(aps.clone()));

    // The up- and download uses a longer timeout than the default 30s on the usual routes.
    // To accomodate very slow clients we assume each MB can take up to a full minute for up- or download.
    // However, the minimum timeout is always set to 120s.
    let file_endpoint_timeout_duration =
        std::cmp::max(120, aps.conf.maximum_filesize as u64 / 17476);
    tracing::info!(
        "setting file endpoint timeout to {} seconds",
        file_endpoint_timeout_duration
    );

    // Define the actual application (routes, middlewares, services).
    let app = Router::new()
        // Serve static assets from the 'static'-folder
        .nest_service("/static", ServeDir::new("static"))
        // HTML routes
        .route("/", get(upload::upload_page))
        .route("/file", get(download::download_page))
        .route("/admin", get(admin::admin_page))
        // API / non-HTML routes
        .route("/admin_login", post(admin::admin_login))
        .route("/admin_logout", post(admin::admin_logout))
        .route("/delete_endpoint", post(delete::delete_endpoint))
        // Normal requests that don't download or upload files should finish in 30s.
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        // Set the upload size limit for the upload_endpoint to the accepted filesize
        // plus a generous 256 KiB for the other metadata.
        // The default limit is 2MB, not enough for most configurations.
        .route(
            "/upload_endpoint",
            post(upload::upload_endpoint)
                .layer(DefaultBodyLimit::max(aps.conf.maximum_filesize + 262144))
                .layer(axum::middleware::from_fn_with_state(
                    aps.clone(),
                    upload::upload_endpoint_wrapper,
                )),
        )
        .route("/download_endpoint", get(download::download_endpoint))
        .layer(TimeoutLayer::new(Duration::from_secs(
            file_endpoint_timeout_duration,
        )))
        // Enable response compression of all responses
        .layer(ServiceBuilder::new().layer(CompressionLayer::new()))
        // Use our custom middleware for rate limiting.
        .layer(middleware::from_fn_with_state(
            aps.clone(),
            ip_prefix::ip_prefix_ratelimiter,
        ))
        // Use our custom middleware for tracing
        .layer(middleware::from_fn(custom_tracing))
        // Attach DB-pool and TERA-object as State
        .with_state(aps)
        // Ensure client IPs and ports can be extracted
        .into_make_service_with_connect_info::<SocketAddr>();

    // Bind to localhost for now.
    let listener = match tokio::net::TcpListener::bind(&interface).await {
        Ok(v) => {
            tracing::info!("listening on {}", &interface);
            v
        }
        Err(e) => {
            tracing::error!("failed to open TcpListener on {}: {}", &interface, e);
            return ExitCode::FAILURE;
        }
    };

    match axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_handler())
        .await
    {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("failed to serve application with axum: {e}");
            ExitCode::FAILURE
        }
    }
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

/// Takes a value in bytes and pretty prints it with a binary suffix.
pub fn pretty_print_bytes(bytes: usize) -> String {
    match bytes {
        0..1_024 => {
            format!("{} Bytes", bytes)
        }
        1_024..1_048_576 => {
            format!("{:.1} KiB", bytes as f64 / 1_024.0)
        }
        1_048_576..1_073_741_824 => {
            format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
        }
        _ => {
            format!("{:.1} GiB", bytes as f64 / 1_073_741_824.0)
        }
    }
}
