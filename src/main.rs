use askama::Template;
use axum::{response::Html, routing::get, Router};
use minify_html::minify;
use tower::ServiceBuilder;
use tower_http::{compression::CompressionLayer, services::ServeDir};
// use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

static HTML_MINIFY_CFG: LazyLock<minify_html::Cfg> = LazyLock::new(|| {
    let mut cfg = minify_html::Cfg::spec_compliant();
    // Keep things compliant, we don't need to crunc *that much*.
    cfg.keep_closing_tags = true;
    cfg.keep_html_and_head_opening_tags = true;
    // Very useful, minify all the CSS here, too.
    cfg.minify_css = true;
    cfg
});

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate<'a> {
    title: &'a str,
}

#[tokio::main]
async fn main() {
    // Define the app's routes.
    let app = Router::new()
        // Main routes
        .route("/", get(root))
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

async fn root() -> Html<String> {
    let h = IndexTemplate {
        title: "Cool Title",
    };
    Html(String::from_utf8(minify(h.render().unwrap().as_bytes(), &HTML_MINIFY_CFG)).unwrap())
}

// async fn create_user(Json(payload): Json<CreateUser>) -> (StatusCode, Json<User>) {
//     let user = User {
//         id: 1337,
//         username: payload.username,
//     };
//
//     println!("{:?}", user);
//
//     (StatusCode::CREATED, Json(user))
// }
//
// #[derive(Deserialize)]
// struct CreateUser {
//     username: String,
// }
//
// #[derive(Serialize, Debug)]
// struct User {
//     id: u64,
//     username: String,
// }
