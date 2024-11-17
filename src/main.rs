use axum::{
    extract::MatchedPath,
    http::{Request, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use sqlx::{migrate::MigrateDatabase, Sqlite, SqlitePool};
use std::{net::SocketAddr, sync::Arc};
use tera::Tera;
use tokio::sync::Mutex;
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, services::ServeDir, trace::TraceLayer};
use tracing::info_span;
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
pub struct AppError(anyhow::Error);

/// Allows axum to automatically convert our custom AppError into a Response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR, 
            format!("Internal server error: {}", self.0),
        ).into_response()
    }
}

/// Ensure that our custom error type can be built automatically from anyhow::Error.
/// This allows us to use the ?-operator in request-handlers to easily handle errors.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

/// Define a custom bail!-macro that includes a call to .into(),
/// automatically converting the anyhow::Error to an AppError.
///
/// Based on advice from this GitHub-issue on the anyhow-crate:
/// <https://github.com/dtolnay/anyhow/issues/112#issuecomment-704549251>
#[macro_export]
macro_rules! bail {
    ($($err:tt)*) => {
        return Err(anyhow::anyhow!($($err)*).into());
    };
}

/// Define a custom ensure!-macro that includes a call to .into(),
/// automatically converting the anyhow::Error to an AppError.
///
/// Based on advice from this GitHub-issue on the anyhow-crate:
/// <https://github.com/dtolnay/anyhow/issues/112#issuecomment-704549251>
#[macro_export]
macro_rules! ensure {
    ($cond:expr, $($err:tt)*) => {
        if !$cond {
            return Err(anyhow::anyhow!($($err)*).into());
        }
    };
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

    // Initialize the templating engine.
    let tera = Arc::new(Mutex::new(
        Tera::new("templates/**/*.{html,js}").expect("error during template parsing"),
    ));

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

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                // axum logs rejections from built-in extractors with the `axum::rejection`
                // target, at `TRACE` level. `axum::rejection=trace` enables showing those events
                format!(
                    "{}=debug,tower_http=debug,axum::rejection=trace",
                    env!("CARGO_CRATE_NAME")
                )
                .into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let aps = AppState { tera, db };

    // Define the app's routes.
    let app = Router::new()
        // Main routes
        .route("/", get(upload::upload_page))
        .route("/file", get(download::download_page))
        .route("/admin", get(admin::admin_get))
        .route("/admin", post(admin::admin_post))
        .route("/upload_endpoint", post(upload::upload_endpoint))
        .route("/download_endpoint", get(download::download_endpoint))
        .route("/delete_endpoint", post(delete::delete_endpoint))
        // Serve static assets from the 'static'-folder.
        .nest_service("/static", ServeDir::new("static"))
        // Enable response compression.
        .layer(ServiceBuilder::new().layer(CompressionLayer::new()))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                // Log the matched route's path (with placeholders not filled in).
                // Use request.uri() or OriginalUri if you want the real path.
                let matched_path = request
                    .extensions()
                    .get::<MatchedPath>()
                    .map(MatchedPath::as_str);

                info_span!(
                    "http_request",
                    method = ?request.method(),
                    matched_path,
                    some_other_field = tracing::field::Empty,
                )
            }),
        )
        .with_state(aps)
        .into_make_service_with_connect_info::<SocketAddr>();

    // Bind to localhost for now.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8000")
        .await
        .unwrap();

    tracing::debug!("listeing on {}", listener.local_addr().unwrap());

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
