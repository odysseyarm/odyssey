use odyssey_hub_client::client;

#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    client::run().await
}
