use anyhow::Ok;
use odyssey_hub_client::client;
use futures::stream::StreamExt;

#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt();
    let mut client = client::Client::default();
    client.connect().await?;
    let mut stream = client.subscribe_events().await?;
    tokio::select! {
        _ = tokio::spawn(async move {
            while let Some(reply) = stream.next().await {
                println!("{:?}", reply);
            }
        }) => {},
        _ = tokio::spawn(async move {
            loop {
                let device_list = client.get_device_list().await.unwrap();
                println!("{:?}", device_list);
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        }) => {},
    }
    Ok(())
}
