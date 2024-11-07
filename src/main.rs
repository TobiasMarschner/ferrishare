use axum::{
    extract::Multipart,
    response::{Html, IntoResponse},
    routing::{get, post},
    Router,
};
use minify_html::minify;
use tera::{Context, Tera};
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, services::ServeDir};
// use serde::{Deserialize, Serialize};
use std::{fs::File, io::prelude::*, sync::{LazyLock, Mutex}};

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

#[tokio::main]
async fn main() {
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
        .layer(ServiceBuilder::new().layer(CompressionLayer::new()));

    // Bind to localhost for now.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8000")
        .await
        .unwrap();

    axum::serve(listener, app).await.unwrap();
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
    let h = TERA.lock().unwrap().render("download.html", &context).unwrap();
    Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap())
}

async fn admin_link() -> impl IntoResponse {
    TERA.lock().unwrap().full_reload().unwrap();
    let context = Context::new();
    let h = TERA.lock().unwrap().render("admin_link.html", &context).unwrap();
    Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap())
}

async fn admin_overview() -> impl IntoResponse {
    TERA.lock().unwrap().full_reload().unwrap();
    let context = Context::new();
    let h = TERA.lock().unwrap().render("admin_overview.html", &context).unwrap();
    Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap())
}

async fn admin() -> impl IntoResponse {
    TERA.lock().unwrap().full_reload().unwrap();
    let context = Context::new();
    let h = TERA.lock().unwrap().render("admin.html", &context).unwrap();
    Html(String::from_utf8(minify(h.as_bytes(), &HTML_MINIFY_CFG)).unwrap())
}

async fn upload_endpoint(mut multipart: Multipart) {
    println!("endpoint reached");
    // dbg!(&multipart);

    while let Some(field) = multipart.next_field().await.unwrap() {
        dbg!(&field);

        let name = field.name().unwrap().to_string();
        let ct = field.content_type().unwrap().to_string();
        let filename = field.file_name().unwrap().to_string();
        let data = field.bytes().await.unwrap();

        println!("Length of `{}` is {} bytes, content-type {}", name, data.len(), ct);

        let mut file = File::create(format!("data/{filename}")).unwrap();
        file.write_all(&data).unwrap();
    }
}

