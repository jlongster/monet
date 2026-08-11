mod app_log;
mod config;
mod db;
mod error;
mod http;
mod mcp;
mod model;
mod operations;
mod perf;
mod plugin;
mod state;

use std::{env, fs, net::SocketAddr, path::Path};

use crate::{config::ConfigFile, db::Database, http::create_router};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_path = env::var("MONET_DB_PATH").unwrap_or_else(|_| ".money-tui/money.db".into());
    if let Some(parent) = Path::new(&database_path).parent() {
        fs::create_dir_all(parent)?;
    }
    let address: SocketAddr = env::var("MONET_SERVER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:47832".into())
        .parse()?;
    let app = create_router(Database::open(&database_path)?, ConfigFile::open()?);

    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("monet server listening on http://{address}");
    app_log::write("server", &format!("listening on http://{address}"));
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
