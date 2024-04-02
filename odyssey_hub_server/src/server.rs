use interprocess::local_socket::{tokio::prelude::{LocalSocketListener, LocalSocketStream}, traits::tokio::{Listener, Stream}, NameTypeSupport, ToFsName, ToNsName};
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, try_join};

pub async fn run() -> anyhow::Result<()> {
    // Describe the things we do when we've got a connection ready.
    async fn handle_conn(conn: LocalSocketStream) -> std::io::Result<()> {
        // Split the connection into two halves to process
        // received and sent data concurrently.
        let (reader, mut writer) = conn.split();
        let mut reader = BufReader::new(reader);

        // Allocate a sizeable buffer for reading.
        // This size should be enough and should be easy to find for the allocator.
        let mut buffer = String::with_capacity(128);

        // Describe the write operation as writing our whole message.
        let write = writer.write_all(b"Hello from server!\n");
        // Describe the read operation as reading into our big buffer.
        let read = reader.read_line(&mut buffer);

        // Run both operations concurrently.
        try_join!(read, write)?;

        // Dispose of our connection right now and not a moment later because I want to!
        drop((reader, writer));

        // Produce our output!
        println!("Client answered: {}", buffer.trim());
        Ok(())
    }

    // Pick a name. There isn't a helper function for this, mostly because it's largely unnecessary:
    // in Rust, `match` is your concise, readable and expressive decision making construct.
    let name = {
        // This scoping trick allows us to nicely contain the import inside the `match`, so that if
        // any imports of variants named `Both` happen down the line, they won't collide with the
        // enum we're working with here. Maybe someone should make a macro for this.
        use NameTypeSupport::*;
        match NameTypeSupport::query() {
            OnlyFs => "/tmp/odyhub.sock".to_fs_name(),
            OnlyNs | Both => "@odyhub.sock".to_ns_name(),
        }
    };
    let name = name?;
    // Create our listener. In a more robust program, we'd check for an
    // existing socket file that has not been deleted for whatever reason,
    // ensure it's a socket file and not a normal file, and delete it.
    let listener = LocalSocketListener::bind(name.clone())?;
    println!("Server running at {:?}", name.clone());

    // Set up our loop boilerplate that processes our incoming connections.
    loop {
        // Sort out situations when establishing an incoming connection caused an error.
        let conn = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("There was an error with an incoming connection: {}", e);
                continue;
            }
        };

        // Spawn new parallel asynchronous tasks onto the Tokio runtime
        // and hand the connection over to them so that multiple clients
        // could be processed simultaneously in a lightweight fashion.
        tokio::spawn(async move {
            // The outer match processes errors that happen when we're
            // connecting to something. The inner if-let processes errors that
            // happen during the connection.
            if let Err(e) = handle_conn(conn).await {
                eprintln!("Error while handling connection: {}", e);
            }
        });
    }
}
