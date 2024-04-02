use interprocess::local_socket::{tokio::prelude::LocalSocketStream, traits::tokio::Stream, NameTypeSupport, ToFsName, ToNsName};
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, try_join};
use tokio_util::sync::CancellationToken;

pub async fn run() -> anyhow::Result<()> {
    let name = {
        use NameTypeSupport::*;
        match NameTypeSupport::query() {
            OnlyFs => "/tmp/odyhub.sock".to_fs_name(),
            OnlyNs | Both => "@odyhub.sock".to_ns_name(),
        }
    };
    let name = name?;

    // Await this here since we can't do a whole lot without a connection.
    let conn = LocalSocketStream::connect(name).await?;

    // This consumes our connection and splits it into two halves,
    // so that we could concurrently act on both.
    let (reader, mut writer) = conn.split();
    let mut reader = BufReader::new(reader);

    // Allocate a sizeable buffer for reading.
    // This size should be enough and should be easy to find for the allocator.
    let mut buffer = String::with_capacity(128);

    let end_token = CancellationToken::new();

    let task = tokio::spawn({
        let end_token = end_token.clone();
        async move {
            while !end_token.is_cancelled() {
                // Describe the write operation as writing our whole string.
                let write = writer.write_all(b"Hello from client!\n");
                // Describe the read operation as reading until a newline into our buffer.
                let read = reader.read_line(&mut buffer);

                // Concurrently perform both operations.
                try_join!(write, read).unwrap();

                // Display the results when we're done!
                println!("Server answered: {}", buffer.trim());

                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
            writer.write_all(b"exit\n").await.unwrap();
            drop((reader, writer));
        }
    });

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.unwrap();
        end_token.cancel();
    }).await?;

    task.await?;

    Ok(())
}
