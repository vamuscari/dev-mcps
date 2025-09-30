use anyhow::Result;
use rmcp::ServiceExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod mcp;
mod codex;
mod protocol_types;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging (env: RUST_LOG=info,debug,trace)
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,codex_orchestrator=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_ansi(false))
        .init();

    tracing::info!("Starting codex-orchestrator MCP server");

    let state = mcp::Orchestrator::new();
    // Serve MCP over stdio using rmcp
    let service = state
        .serve(rmcp::transport::stdio())
        .await
        .inspect_err(|e| tracing::error!(error=?e, "serving error"))?;

    // Share upstream peer so background tasks can send notifications.
    mcp::set_upstream_peer(service.peer().clone());

    // Wait until the service finishes (e.g., on shutdown)
    service.waiting().await?;
    Ok(())
}
