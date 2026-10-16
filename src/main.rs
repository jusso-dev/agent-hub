use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let admin_key = std::env::var("HUB_ADMIN_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .ok_or("HUB_ADMIN_KEY is required")?;
    let db_path = std::env::var("HUB_DB").unwrap_or_else(|_| "hub.db".into());
    let bind = std::env::var("HUB_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());

    let pool = agent_hub::db::connect(&db_path).await?;
    let app = agent_hub::app(agent_hub::AppState::new(pool, admin_key));

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("agent-hub listening on http://{bind}");
    axum::serve(listener, app).await?;
    Ok(())
}
