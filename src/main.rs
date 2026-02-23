use rmcp::{ServiceExt, transport::stdio};

use squall::config::Config;
use squall::server::SquallServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    // Load .env from the binary's directory (MCP servers may start with any CWD).
    // Falls back to dotenvy's default CWD search if the binary path can't be resolved.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let env_path = dir.join(".env");
            if env_path.exists() {
                dotenvy::from_path(&env_path).ok();
            } else {
                // Try the cargo project root (development builds: target/release/../..)
                let project_root = dir.join("../../.env");
                if project_root.exists() {
                    dotenvy::from_path(&project_root).ok();
                } else {
                    dotenvy::dotenv().ok();
                }
            }
        } else {
            dotenvy::dotenv().ok();
        }
    } else {
        dotenvy::dotenv().ok();
    }

    tracing::info!("squall starting");

    let config = Config::load();
    let server = SquallServer::new(config);

    let service = server
        .serve(stdio())
        .await
        .inspect_err(|e| tracing::error!("serving error: {e:?}"))?;

    service.waiting().await?;

    tracing::info!("squall shutting down");
    Ok(())
}
