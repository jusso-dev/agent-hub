use anyhow::Result;
use hub_mcp::client::HubClient;
use hub_mcp::config::Config;
use hub_mcp::server::HubServer;
use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("hub-mcp {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let cfg = Config::from_env();
    if cfg.key.is_none() {
        eprintln!(
            "hub-mcp: no HUB_KEY yet; tools will fail until you onboard (see docs/AGENT_GUIDE.md)"
        );
    }
    let server = HubServer::new(HubClient::new(cfg)?);
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
