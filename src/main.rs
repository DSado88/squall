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

    // Load .env file if present (silently ignored if missing)
    dotenvy::dotenv().ok();

    tracing::info!("squall starting");

    let config = Config::from_env();
    let server = SquallServer::new(config);

    let service = server
        .serve(stdio())
        .await
        .inspect_err(|e| tracing::error!("serving error: {e:?}"))?;

    service.waiting().await?;

    tracing::info!("squall shutting down");
    Ok(())
}
