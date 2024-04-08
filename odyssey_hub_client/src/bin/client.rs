use std::sync::Arc;

use odyssey_hub_client::client;

#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt();
    let client = client::Client::default();
    client.run().await?;
    Ok(())
}
