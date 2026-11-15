//! Prints the hub's device list and then every event it streams, for debugging.
#![expect(clippy::print_stdout, reason = "this debugging tool prints to stdout")]

use futures::stream::StreamExt as _;
use odyssey_hub_client::client;

#[tokio::main]
/// # Errors
/// Returns an error if connecting to the hub or an RPC fails.
///
/// # Panics
/// Panics if Tokio runtime initialization fails.
pub async fn main() -> eros::Result<()> {
    let _ = tracing_subscriber::fmt();
    let client = client::Client::connect().await?;
    let device_list = client.get_device_list().await?;
    println!("{device_list:?}");
    let mut stream = client.subscribe_events().await?;
    while let Some(reply) = stream.next().await {
        println!("{reply:?}");
    }
    Ok(())
}
