use interprocess::local_socket::{tokio::prelude::LocalSocketStream, traits::tokio::Stream, NameTypeSupport, ToFsName, ToNsName};
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, try_join};

#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
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

    // Describe the write operation as writing our whole string.
    let write = writer.write_all(b"Hello from client!\n");
    // Describe the read operation as reading until a newline into our buffer.
    let read = reader.read_line(&mut buffer);

    // Concurrently perform both operations.
    try_join!(write, read)?;

    // Close the connection a bit earlier than you'd think we would. Nice practice!
    drop((reader, writer));

    // Display the results when we're done!
    println!("Server answered: {}", buffer.trim());

    Ok(())
}
