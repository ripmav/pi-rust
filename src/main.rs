mod agent;
mod cli;
mod client;
mod model;
mod session;
mod provider;
mod tools;
mod transport;

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
