use axum::serve;
use clap::{ArgAction, Parser};
use config::Config;
use eyre::Result;
use service::service;
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
};
use tokio::net::TcpListener;
use tracing::info;
use user::{Authenticator, User};

mod config;
mod service;
mod user;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    config: PathBuf,

    #[arg(short, long, action = ArgAction::Count)]
    verbose: u8,
}

fn setup_logs(args: &Args) {
    let level = match args.verbose {
        0 => tracing::Level::INFO,
        1 => tracing::Level::DEBUG,
        _ => tracing::Level::TRACE,
    };
    tracing_subscriber::fmt().with_max_level(level).init()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    setup_logs(&args);

    let config = Config::new(&args.config)?;

    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, config.port));
    info!("Listening on {}", addr);

    let listener = TcpListener::bind(addr).await?;
    serve(listener, service(&config)).await?;

    Ok(())
}
