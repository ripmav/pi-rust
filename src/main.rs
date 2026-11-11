mod agent;
mod auth;
mod cli;
mod edit_diff;
mod client;
mod config;
mod corelog;
mod model;
mod session;
mod templates;
mod provider;
mod registry;
mod tools;
mod transport;
mod tui;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    match cli::run().await {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("[core] fatal: {e:#}");
            std::process::exit(1);
        }
    }
}
