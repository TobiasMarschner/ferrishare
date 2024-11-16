use axum::{
    routing::{get, post},
    Router,
};
use sqlx::{migrate::MigrateDatabase, Sqlite, SqlitePool};
use std::{
    net::SocketAddr,
    sync::{LazyLock, Mutex},
};
use tera::Tera;
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, services::ServeDir};

mod admin;
mod download;
mod upload;
mod delete;

pub static HTML_MINIFY_CFG: LazyLock<minify_html::Cfg> = LazyLock::new(|| {
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

    // Define the app's routes.
    let app = Router::new()
        // Main routes
        .route("/", get(upload::upload_page))
        .route("/file", get(download::download_page))
        .route("/admin", get(admin::admin_get))
        .route("/admin", post(admin::admin_post))
        .route("/admin_link", get(admin::admin_link))
        .route("/admin_overview", get(admin::admin_overview))
        .route("/upload_endpoint", post(upload::upload_endpoint))
        .route("/download_endpoint", get(download::download_endpoint))
        .route("/delete_endpoint", post(delete::delete_endpoint))
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
