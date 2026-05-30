use axum::serve::serve;
use config::Config;
use eyre::Result;
use service::service;
use std::{net::SocketAddr, sync::Arc};
use tokio::{net::TcpListener, spawn};
use tracing::debug;

mod config;
mod service;
mod user;

async fn diagnostics_service(config: Arc<Config>) -> Result<()> {
    let address = config.diagnostics.config_endpoint.bind_address.value;

    let listener = TcpListener::bind(address).await?;

    debug!(%address, "exposing diagnostics endpoint");
    serve(listener, config::service(config)).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    simple_eyre::install()?;

    // Config::load() parses CLI flags (which also read env vars), optionally
    // reads a TOML file, merges all three layers, and resolves defaults.
    let config = Arc::new(config::load(std::env::args_os())?);

    // Set up tracing. The level already incorporates any -v / --verbose
    // overrides applied during Config::load().
    tracing_subscriber::fmt()
        .with_max_level(config.logging.level.value)
        .init();

    if config.diagnostics.config_endpoint.enabled.value {
        spawn(diagnostics_service(Arc::clone(&config)));
    }

    let address = SocketAddr::new(config.server.bind_address.value, config.server.port.value);
    let listener = TcpListener::bind(address).await?;

    debug!(%address, "exposing service endpoint");
    serve(listener, service(config)).await?;

    Ok(())
}
