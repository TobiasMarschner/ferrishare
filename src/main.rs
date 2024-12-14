//! Simple, self-hostable filesharing application with builtin end-to-end encryption
//!
//! This crate is not usable as-is, but is instead part of a larger application package.  
//! Check the project repository's README for details on how to install and use FerriShare:  
//! <https://github.com/TobiasMarschner/ferrishare>

use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, State},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use clap::Parser;
use itertools::Itertools;
use minify_html::minify;
use sqlx::{migrate::MigrateDatabase, FromRow, Sqlite, SqlitePool};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    process::ExitCode,
    sync::Arc,
    time::Duration,
};
use tera::Tera;
use tokio::sync::RwLock;
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

/// The application's global state that is passed to every request handler
#[derive(Debug, Clone)]
pub struct AppState {
    /// The global TERA instance responsible for HTML and JS templating
    tera: Arc<Tera>,
    /// The global SqlitePool responsible for making queries to the SQLite-database
    ///
    /// [SqlitePool] is internally wrapped in an [Arc], so no need to wrap it here
    db: SqlitePool,
    /// Immutable global configuration for FerriShare, read during startup from {DATA_PATH}/config.toml
    conf: Arc<AppConfiguration>,
    /// Table keeping track of the number of requests made by each IpPrefix for rate limiting
    rate_limiter: Arc<RwLock<HashMap<IpPrefix, u64>>>,
    /// Set of IpPrefixes that are uploading a file at this moment
    ///
    /// Any given IpPrefix is only allowed to stream one file at a time.
    /// Otherwise, a malicious client could start hundreds of uploads
    /// simultaneously and bypass quota restrictions.
    uploading: Arc<RwLock<HashSet<IpPrefix>>>,
}

impl AppState {
    /// Create the default TERA templating context containing variables needed on every page
    pub fn default_context(&self) -> tera::Context {
        let mut context = tera::Context::new();
        context.insert("global_app_name", &self.conf.app_name);
        context.insert("enable_privacy_policy", &self.conf.enable_privacy_policy);
        context.insert("enable_legal_notice", &self.conf.enable_legal_notice);
        context.insert("demo_mode", &self.conf.demo_mode);
        context.insert("global_crate_version", env!("CARGO_PKG_VERSION"));
        context.insert("global_git_hash", option_env!("VCS_REF").unwrap_or("dev"));
        context
    }
}

/// Global definition of the HTML-minifier configuration
///
/// CSS- and JS-minification are enabled, while some more aggressive
/// and non-compliant settings for HTML minifacation have been disabled.
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
/// - User templates in 'user_templates/'
const DATA_PATH: &str = "./data";

/// Path to the application's SQLite-database
const DB_URL: &str = "sqlite://data/sqlite.db";

/// Custom middleware for tracing HTTP requests.
///
/// I have intentionally chosen not to use tower_http::trace::TraceLayer.
/// Ultimately, this boils down to the fact that http::Request does not contain the
/// connect client's IP address and port (the SocketAddr).
/// Instead, this info has to be extracted using the ConnectInfo extractor provided by axum.
///
/// The middleware creates an "http_request" span wrapping the entire request and
/// fires off an event at the beginning which is then logged by the fmt Subscriber.
async fn custom_tracing(
    State(_): State<AppState>,
    ExtractIpPrefix(eip): ExtractIpPrefix,
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
    let method = request.method();

    // Create the http_request span out of this info.
    let span = tracing::info_span!("http_request", socket_address = %client, real_ip = eip.pretty_print(), path, query, ?method);

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

/// Add a 'Cache-Control' header to a response that causes it to be cached permanently.
async fn add_maximum_caching<B>(mut response: Response<B>) -> Response<B> {
    response.headers_mut().insert(
        "Cache-Control",
        "max-age=31536000, immutable".parse().unwrap(),
    );
    response
}

/// Simple, self-hostable filesharing application with builtin end-to-end-encryption
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

/// The application's main starting point
#[tokio::main]
async fn main() -> ExitCode {
    // First things first, create the DATA_PATH and its subdirectories.
    std::fs::create_dir_all(format!("{DATA_PATH}/uploaded_files"))
        .and_then(|_| std::fs::create_dir_all(format!("{DATA_PATH}/user_templates")))
        .unwrap_or_else(|e| {
            panic!("failed to create configuration and application data directories at {DATA_PATH}: {e}")
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
            eprintln!("  docker compose run --rm -it ferrishare --init  (for Docker Compose)");
            eprintln!("  cargo run --release -- --init                  (for cargo)");

            eprintln!("\nExiting!");
            return ExitCode::FAILURE;
        }
    };

    let app_config: AppConfiguration = match toml::from_str(&config_string) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Failed to parse configuration file at {DATA_PATH}/config.toml: {e}");

            eprintln!("\nIf your config file is causing trouble, consider regenerating it by running the app with the '--init' flag:");
            eprintln!("  docker compose run --rm -it ferrishare --init  (for Docker Compose)");
            eprintln!("  cargo run --release -- --init                  (for cargo)");

            eprintln!("\nExiting!");
            return ExitCode::FAILURE;
        }
    };

    // Set up `tracing` (logging).
    // Use the default formatting subscriber provided by `tracing_subscriber`.
    // The log level is provided by the configuration.
    tracing_subscriber::fmt()
        .with_max_level(app_config.translate_log_level())
        .init();

    tracing::info!("read config from {DATA_PATH}/config.toml");

    // Create the database if it doesn't already exist.
    if !Sqlite::database_exists(DB_URL).await.unwrap_or(false) {
        tracing::info!("could not locate sqlite-db! creating a new one ...");
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

    // Initialize the templating engine.
    let tera = match Tera::new("templates/**/*.{html,js}").and_then(|mut v| {
        match v.add_template_files([
            (
                format!("{DATA_PATH}/user_templates/privacy_policy.html"),
                Some("privacy_policy.html"),
            ),
            (
                format!("{DATA_PATH}/user_templates/legal_notice.html"),
                Some("legal_notice.html"),
            ),
        ]) {
            Ok(_) => Ok(v),
            Err(e) => Err(e),
        }
    }) {
        Ok(t) => {
            tracing::info!("successfully loaded and compiled HTML and JS templates");
            t
        }
        Err(e) => {
            tracing::error!("failed to load and compile HTML and JS templates: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Wrap it in an Arc<_>, as required by AppState.
    let tera = Arc::new(tera);

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

    // Create all of the middlewares the app uses.

    // Small timeouts for all the "normal" routes that don't deal with files.
    let timeout_small = TimeoutLayer::new(Duration::from_secs(30));

    // Big timeout for file uploads and downloads.
    // The up- and download uses a longer timeout than the default 30s on the usual routes.
    // To accomodate very slow clients we assume each MB can take up to a full minute for up- or download.
    // However, the minimum timeout is always set to 120s.
    let file_endpoint_timeout_duration =
        std::cmp::max(120, aps.conf.maximum_filesize as u64 / 17476);
    let timeout_big = TimeoutLayer::new(Duration::from_secs(file_endpoint_timeout_duration));
    tracing::info!(
        "setting file endpoint timeout to {} seconds",
        file_endpoint_timeout_duration
    );

    // Compression for HTTP responses.
    // Will not be used on static font assets (since they're already compressed)
    // and file up- and downloads (since they're encrypted and thereby not practically compressible).
    // We're disabling zstd because brotli and gzip have proven to provide better results thus far.
    let compression = CompressionLayer::new().no_zstd();

    // Our custom middleware for tracing HTTP requests.
    let custom_tracing = middleware::from_fn_with_state(aps.clone(), custom_tracing);

    // Our custom middleware for rate-limiting with the IpPrefix.
    let rate_limiter =
        middleware::from_fn_with_state(aps.clone(), ip_prefix::ip_prefix_ratelimiter);

    // Adds a cache header for infinite caching of resources.
    // Make sure all resources served here have hashes included in their request path.
    let permanent_caching = middleware::map_response(add_maximum_caching);

    // Set the upload size limit for the upload_endpoint to the accepted filesize
    // plus a generous 256 KiB for the other metadata.
    // The default limit is 2MB, not enough for most configurations.
    let upload_endpoint_limit: usize = match (aps.conf.maximum_filesize + 262144).try_into() {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("maximum filesize exceeds computer's bit-width: {e}");
            return ExitCode::FAILURE;
        },
    };
    // Routers for up- and downloading the actual file payloads
    let file_routers = Router::new()
        .route(
            "/upload_endpoint",
            post(upload::upload_endpoint)
                .layer(DefaultBodyLimit::max(upload_endpoint_limit))
                // This ensures any IpPrefix can only upload one file at a time.
                .layer(axum::middleware::from_fn_with_state(
                    aps.clone(),
                    upload::upload_endpoint_wrapper,
                )),
        )
        .route("/download_endpoint", get(download::download_endpoint))
        .layer(timeout_big);

    // The usual frontend routes
    let mut normal_routers = Router::new()
        // HTML routes
        .route("/", get(upload::upload_page))
        .route("/file", get(download::download_page))
        .route("/admin", get(admin::admin_page))
        // API / non-HTML routes
        .route("/admin_login", post(admin::admin_login))
        .route("/admin_logout", post(admin::admin_logout))
        .route("/delete_endpoint", post(delete::delete_endpoint));

    // Add Privacy Policy / Legal Notice, if configured.
    if aps.conf.enable_privacy_policy {
        normal_routers = normal_routers.route("/privacy-policy", get(privacy_policy));
    }
    if aps.conf.enable_legal_notice {
        normal_routers = normal_routers.route("/legal-notice", get(legal_notice));
    }

    // Add middlewares for the normal routes.
    let normal_routers = normal_routers
        .layer(timeout_small)
        .layer(compression.clone());

    // Static assets are compressed and permanently cached. (like the main.css bundle)
    let static_routers = Router::new()
        .nest_service("/static", ServeDir::new("static"))
        .layer(timeout_small)
        .layer(compression)
        .layer(permanent_caching.clone());

    // Fonts and icons are permanently cached, but not compressed.
    let font_routers = Router::new()
        .nest_service("/font", ServeDir::new("font"))
        .nest_service("/favicon", ServeDir::new("favicon"))
        .layer(timeout_small)
        .layer(permanent_caching);

    // Combine all Routers into one big router and add the global middlewares and state here.
    // Logging and rate-limiting apply to all routes indiscriminately.
    let app = Router::new()
        .nest("/", normal_routers)
        .nest("/", file_routers)
        .nest("/", font_routers)
        .nest("/", static_routers)
        .layer(custom_tracing)
        .layer(rate_limiter)
        .with_state(aps)
        .into_make_service_with_connect_info::<SocketAddr>();

    // Bind a TcpListener to the interface specified in the config.
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

    // And, finally, start serving requests
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

/// Ensure CTRL+C and SIGTERM cause the application to gracefully shut down.
async fn shutdown_handler() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install CTRL+C handler");
    };

    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("received shutdown signal");
}

/// Simple handler for the Privacy Policy
///
/// Should only be inserted into the Router if the config enables the Privacy Policy.
async fn privacy_policy(State(aps): State<AppState>) -> Result<impl IntoResponse, AppError> {
    let context = aps.default_context();
    let html = aps.tera.render("privacy_policy.html", &context)?;
    let response_body = String::from_utf8(minify(html.as_bytes(), &MINIFY_CFG))?;
    Ok(Html(response_body))
}

/// Simple handler for the Legal Notice
///
/// Should only be inserted into the Router if the config enables the Legal Notice.
async fn legal_notice(State(aps): State<AppState>) -> Result<impl IntoResponse, AppError> {
    let context = aps.default_context();
    let html = aps.tera.render("legal_notice.html", &context)?;
    let response_body = String::from_utf8(minify(html.as_bytes(), &MINIFY_CFG))?;
    Ok(Html(response_body))
}

/// Returns true if the expiry_ts lies in the past, i.e. the resource has expired.
///
/// Remember that uploaded files are not cleaned up immediately, but are instead deleted
/// every 15 minutes when the cleanup task wakes up. Sometimes files may have officially
/// expired but are still present on disk and in the database.
///
/// This function checks whether a file should be served or treated as "already deleted".
pub fn has_expired(expiry_ts: &str) -> Result<bool, AppError> {
    Ok(chrono::DateTime::parse_from_rfc3339(expiry_ts)?
        .signed_duration_since(chrono::Utc::now())
        .num_seconds()
        .is_negative())
}

/// Takes a value in bytes and pretty prints it with a binary suffix.
pub fn pretty_print_bytes(bytes: u64) -> String {
    match bytes {
        0..1_024 => {
            format!("{} Bytes", bytes)
        }
        1_024..1_048_576 => {
            format!("{:.2} KiB", bytes as f64 / 1_024.0)
        }
        1_048_576..1_073_741_824 => {
            format!("{:.2} MiB", bytes as f64 / 1_048_576.0)
        }
        _ => {
            format!("{:.2} GiB", bytes as f64 / 1_073_741_824.0)
        }
    }
}
