use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::{self, StreamExt as _};
use futures_concurrency::stream::Merge as _;
use odyssey_hub_server_interface::service_server::ServiceServer;
use tokio::sync::{RwLock, broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::accessories::accessory_manager;
use crate::bind_local_socket::bind_local_socket;
use crate::device_tasks::{self, device_tasks};
use crate::message_forwarder::MessageForwarder;
use crate::server::HubServer;
use crate::startup_config::StartupConfig;
use crate::{Message, ServerError};

/// Capacity of the channel the device tasks report through.
const DEVICE_MESSAGE_CAPACITY: usize = 12;

/// Capacity of the low-level event broadcast. Tracking events arrive fast.
const EVENT_BROADCAST_CAPACITY: usize = 256;

const DEVICE_LIST_CHANGE_CAPACITY: usize = 12;
const DONGLE_LIST_CHANGE_CAPACITY: usize = 16;
const ACCESSORY_MAP_CAPACITY: usize = 12;

/// Which of the hub's long-running parts ended, as seen by the supervising loop.
#[derive(Debug)]
enum ServerEvent {
    DeviceTasks,
    AccessoryManager,
    /// A device task message was applied to the device and dongle lists.
    MessageForwarded,
    TonicServer,
}

/// Runs the hub: the gRPC server on the local socket, the device tasks, the accessory
/// manager and the forwarding of device task messages into the shared lists.
///
/// Returns once the gRPC server or the device tasks end, or `cancel_token` fires.
pub async fn run_server(
    sender: mpsc::UnboundedSender<Message>,
    cancel_token: CancellationToken,
) -> Result<(), ServerError> {
    let (name, connections) = bind_local_socket(&sender).await?;

    let (device_sender, device_messages) = mpsc::channel(DEVICE_MESSAGE_CAPACITY);
    let (event_sender, _) = broadcast::channel(EVENT_BROADCAST_CAPACITY);
    let (device_list_change_sender, _) = broadcast::channel::<()>(DEVICE_LIST_CHANGE_CAPACITY);
    let (dongle_list_change_sender, _) = broadcast::channel::<()>(DONGLE_LIST_CHANGE_CAPACITY);
    let (accessory_map_sender, _) = broadcast::channel(ACCESSORY_MAP_CAPACITY);

    // The guard stops the screen watcher if run_server returns without a cancel.
    let screen_watch_cancel = cancel_token.child_token();
    let _screen_watch_stop = screen_watch_cancel.clone().drop_guard();
    let config = StartupConfig::load(screen_watch_cancel).await;

    let shot_delay_watch = Arc::new(RwLock::new(HashMap::new()));
    let (accessory_info_sender, accessory_info_receiver) =
        watch::channel(config.accessory_info.clone());
    let accessory_map = Arc::new(std::sync::Mutex::new(
        config
            .accessory_info
            .into_iter()
            .map(|(id, info)| (id, (info, false)))
            .collect(),
    ));

    let server = HubServer {
        device_list: Default::default(),
        event_sender: event_sender.clone(),
        device_list_change_sender: device_list_change_sender.clone(),
        screen_calibrations: Arc::clone(&config.screen_calibrations),
        device_shot_delays: Arc::clone(&config.device_shot_delays),
        shot_delay_watch: Arc::clone(&shot_delay_watch),
        accessory_map,
        accessory_map_sender: accessory_map_sender.clone(),
        dongle_list: Default::default(),
        dongle_list_change_sender: dongle_list_change_sender.clone(),
        app_message_sender: sender.clone(),
        accessory_info_sender,
    };

    // The message forwarder is the only writer of the lists the server reads.
    let forwarder = MessageForwarder {
        device_list: server.device_list.clone(),
        dongle_list: server.dongle_list.clone(),
        device_list_changed: device_list_change_sender,
        dongle_list_changed: dongle_list_change_sender,
    };
    let device_list = server.device_list.clone();

    if let Err(e) = sender.send(Message::ServerInit(Ok(config.corruptions))) {
        tracing::warn!("Failed to send server init message: {}", e);
    }
    tracing::info!("Server running at {name:?}");

    // Each of the hub's parts is a stream that yields one tagged event when it ends (the
    let device_task_events = event_sender.clone();
    // message forwarder: one per message), so one loop supervises all of them.
    let device_tasks_stream = stream::once(async move {
        tracing::info!("device_tasks_stream: starting");
        let result = device_tasks(
            device_sender,
            config.screen_calibrations,
            config.device_offsets,
            config.device_shot_delays,
            shot_delay_watch,
            device_task_events,
        )
        .await;
        tracing::error!("device_tasks_stream: device_tasks returned {:?}", result);
        ServerEvent::DeviceTasks
    });

    let accessory_stream = stream::once(async move {
        accessory_manager(
            event_sender,
            accessory_info_receiver,
            accessory_map_sender,
            device_list,
        )
        .await;
        ServerEvent::AccessoryManager
    });

    let message_stream = stream::unfold(
        (device_messages, forwarder),
        |(mut messages, forwarder): (mpsc::Receiver<device_tasks::Message>, _)| async move {
            let message = messages.recv().await?;
            forwarder.forward(message);
            Some((ServerEvent::MessageForwarded, (messages, forwarder)))
        },
    );

    let tonic_stream = stream::once(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(ServiceServer::new(server))
            .serve_with_incoming_shutdown(connections, cancel_token.cancelled())
            .await;
        ServerEvent::TonicServer
    });

    tracing::info!("run_server: merging streams");
    let merged = (
        device_tasks_stream,
        accessory_stream,
        message_stream,
        tonic_stream,
    )
        .merge();
    tokio::pin!(merged);
    tracing::info!("run_server: entering main loop");

    // Wait for any stream to complete (which means a critical task exited)
    while let Some(event) = merged.next().await {
        tracing::debug!("Server loop event: {:?}", event);
        match event {
            ServerEvent::DeviceTasks => {
                tracing::error!("Device tasks exited unexpectedly");
                break;
            }
            ServerEvent::AccessoryManager => {
                tracing::warn!("Accessory manager exited (Bluetooth unavailable?)");
            }
            ServerEvent::MessageForwarded => tracing::trace!("Event forwarded"),
            ServerEvent::TonicServer => {
                tracing::info!("Tonic server exited");
                break;
            }
        }
    }

    tracing::info!("Server main loop ended");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both apps hand `run_server` to `tokio::spawn`, so a non-`Send` value held
    /// across an await inside it breaks them at compile time.
    #[test]
    fn run_server_future_is_send() {
        fn assert_send<T: Send>(_: T) {}
        let (sender, _receiver) = mpsc::unbounded_channel();
        assert_send(run_server(sender, CancellationToken::new()));
    }
}
