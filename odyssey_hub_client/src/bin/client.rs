use anyhow::Ok;
use odyssey_hub_client::client;

#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt();
    let mut client = client::Client::default();
    client.connect().await?;
    let mut stream = client.poll().await?;
    tokio::select! {
        _ = tokio::spawn(async move {
            while let Some(reply) = stream.message().await.unwrap() {
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
