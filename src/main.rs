use axum::serve;
use config::Config;
use eyre::Result;
use service::service;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::info;

mod config;
mod service;
mod user;

#[tokio::main]
async fn main() -> Result<()> {
    // Config::load() parses CLI flags (which also read env vars), optionally
    // reads a TOML file, merges all three layers, and resolves defaults.
    let config = Config::load()?;

    // Set up tracing. The level already incorporates any -v / --verbose
    // overrides applied during Config::load().
    let level: tracing::Level = config.logging.level.into();
    tracing_subscriber::fmt().with_max_level(level).init();

    let addr = SocketAddr::new(config.server.bind_address, config.server.port);
    info!("server starting on {}", addr);

    let listener = TcpListener::bind(addr).await?;
    serve(listener, service(&config)).await?;

    Ok(())
}
