use std::sync::Arc;

use odyssey_hub_client::client;

#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    let client = Arc::new(client::Client::default());
    let res = tokio::try_join!(
        tokio::spawn({ let client = client.clone(); async move { client.run().await } }),
        tokio::spawn({
            let end_token = client.end_token.clone();
            async move {
                tokio::signal::ctrl_c().await.unwrap();
                end_token.cancel();
            }
        })
    );
    match res {
        Ok((_, _)) => {
            Ok(())
        }
        Err(err) => {
           anyhow::bail!("processing failed; error = {}", err);
        }
    }
}
