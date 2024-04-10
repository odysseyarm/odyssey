use std::sync::Arc;

use odyssey_hub_client::client;

#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt();
    let mut client = client::Client::default();
    client.connect().await?;
    loop {
        let device_list = client.get_device_list().await?;
        println!("{:?}", device_list);
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}
