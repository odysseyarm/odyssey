use anyhow::Ok;
use futures::stream::StreamExt;
use odyssey_hub_client::client;

#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt();
    let mut client = client::Client::default();
    client.connect().await?;
    let device_list = client.get_device_list().await.unwrap();
    println!("{:?}", device_list);
    let mut stream = client.subscribe_events().await?;
    while let Some(reply) = stream.next().await {
        println!("{:?}", reply);
    }
    Ok(())
}
