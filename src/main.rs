use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};

use tracing::info;

mod app;
mod error;
mod models;
mod search;
mod util;

use app::AppState;
use search::SearchService;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "reverse_tag_lookup=info,tower_http=info".to_string()),
        )
        .init();

    let cache_path = std::env::var("CACHE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data/problem-cache.json"));

    let state = Arc::new(AppState::new(SearchService::new(cache_path).await?));
    let router = app::router(state);

    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3000);
    let host = std::env::var("HOST")
        .ok()
        .and_then(|value| value.parse::<IpAddr>().ok())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let address = SocketAddr::from((host, port));

    info!("listening on http://{}", address);

    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
