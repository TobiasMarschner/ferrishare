use axum::{
    extract::{ConnectInfo, MatchedPath},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use sqlx::{migrate::MigrateDatabase, Sqlite, SqlitePool};
use std::{net::SocketAddr, sync::Arc};
use tera::Tera;
use tokio::sync::Mutex;
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, services::ServeDir, trace::TraceLayer};
use tracing::{info_span, Instrument, Level};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod admin;
mod delete;
mod download;
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

/// Use a custom error type that can be returned by handlers.
///
/// This follows recommendations from the axum documentation:
/// <https://github.com/tokio-rs/axum/blob/main/examples/anyhow-error-response/src/main.rs>
pub struct AppError {
    status_code: StatusCode,
    message: String,
}

impl AppError {
    /// Create a new Result<T, AppError>::Err with the corresponding StatusCode and message.
    ///
    /// Useful for quickly returning a custom error in any of the request handlers.
    fn err<T>(status_code: StatusCode, message: impl Into<String>) -> Result<T, Self> {
        Err(Self {
            status_code,
            message: message.into(),
        })
    }

    /// Create a new AppError with the corresponding StatusCode and message.
    fn new(status_code: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status_code,
            message: message.into(),
        }
    }

    /// Create a new AppError with the corresponding message and StatusCode 500.
    fn new500(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

/// Allows axum to automatically convert our custom AppError into a Response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if self.status_code.is_client_error() {
            tracing::warn!(status_code = self.status_code.to_string(), self.message);
        } else if self.status_code.is_server_error() {
            tracing::error!(status_code = self.status_code.to_string(), self.message);
        }
        (
            self.status_code,
            format!("{}: {}", self.status_code.to_string(), self.message),
        )
            .into_response()
    }
}

/// Ensure that our custom error type can be built automatically from anyhow::Error.
/// This allows us to use the ?-operator in request-handlers to easily handle errors.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.into().to_string(),
        }
    }
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

const DB_URL: &str = "sqlite://sqlite.db";

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
    let query = request.uri().query();
    // let version = request.version();
    let method = request.method();
    let cookies = request
        .headers()
        .get("cookie")
        .map(|v| v.to_str().unwrap_or_default())
        .unwrap_or_default();

    // Create the http_request span out of this info.
    let span = tracing::info_span!("http_request", %client, path, query, ?method, ?cookies);

    // Instrument the rest of the stack with this span.
    async move {
        // Fire off an event for the received request.
        tracing::info!("received request");
        // And actually process the request.
        next.run(request).await
    }
    .instrument(span)
    .await
}

#[tokio::main]
async fn main() {
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

    let aps = AppState { tera, db };

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
