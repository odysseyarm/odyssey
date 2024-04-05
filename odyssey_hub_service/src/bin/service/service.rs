use std::{ffi::OsString, time::Duration};

use interprocess::local_socket::{tokio::prelude::{LocalSocketListener, LocalSocketStream}, traits::tokio::{Listener, Stream}, NameTypeSupport, ToFsName, ToNsName};
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, sync::mpsc, try_join};

#[cfg(target_os = "windows")]
use windows_service::define_windows_service;
use windows_service::{service::{ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType}, service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle}, service_dispatcher};

#[cfg(target_os = "windows")]
define_windows_service!(ffi_service_main, service_main);

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn start() {
    service_main(vec![]);
}

#[cfg(target_os = "windows")]
pub fn start() {
    match service_dispatcher::start("Odyssey", ffi_service_main) {
        Ok(_) => (),
        Err(e) => eprintln!("Error starting service: {:?}", e),
    }
}

enum Message {
    ServerInit(Result<(), std::io::Error>),
    Stop,
}

#[tokio::main]
async fn service_main(_arguments: Vec<OsString>) -> Result<(), windows_service::Error> {
    let (sender, receiver) = mpsc::unbounded_channel();

    let event_handler = {
        let sender = sender.clone();
        move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                ServiceControl::Stop => {
                    // Handle stop event and return control back to the system.
                    sender.send(Message::Stop).unwrap();
                    ServiceControlHandlerResult::NoError
                }
                // All services must accept Interrogate even if it's a no-op.
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        }
    };

    // Register system service event handler
    let status_handle = service_control_handler::register("OdysseyService", event_handler)?;

    let running_status = ServiceStatus {
        // Should match the one from system service registry
        service_type: ServiceType::OWN_PROCESS,
        // The new state
        current_state: ServiceState::Running,
        // Accept stop events when running
        controls_accepted: ServiceControlAccept::STOP,
        // Used to report an error when starting or stopping only, otherwise must be zero
        exit_code: ServiceExitCode::Win32(0),
        // Only used for pending states, otherwise must be zero
        checkpoint: 0,
        // Only used for pending states, otherwise must be zero
        wait_hint: Duration::default(),
        // Unused for setting status
        process_id: None,
    };

    tokio::select! {
        _ = tokio::spawn(run_service(sender)) => {},
        _ = tokio::spawn({
                let running_status = running_status.clone();
                async move {
                    handle_service_status(status_handle, receiver, running_status).await;
                }
            }) => {}
    };

    // Tell the system that service has stopped.
    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    Ok(())
}

async fn handle_service_status(
    status_handle: ServiceStatusHandle,
    mut receiver: mpsc::UnboundedReceiver<Message>,
    running_status: ServiceStatus,
) {
    loop {
        match receiver.recv().await {
            Some(Message::ServerInit(Ok(()))) => {
                status_handle.set_service_status(running_status.clone()).unwrap();
            }
            Some(Message::ServerInit(Err(_))) => {
                break;
            }
            Some(Message::Stop) => {
                break;
            }
            None => {
                break;
            }
        }
    }
}

async fn run_service(sender: mpsc::UnboundedSender<Message>) -> anyhow::Result<()> {
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

    sender.send(Message::ServerInit(Ok(()))).unwrap();

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
