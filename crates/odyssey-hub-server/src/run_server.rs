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
    DeviceTasks(Result<(), std::convert::Infallible>),
    /// A device task message was applied to the device and dongle lists.
    MessageForwarded,
    TonicServer(Result<(), tonic::transport::Error>),
}

/// How the hub finds its devices. The default is what a user's hub does.
#[derive(Clone, Copy, Debug, Default)]
pub struct ServerOptions {
    /// Run simulated units and dongles next to the physical ones.
    #[cfg(feature = "sim")]
    pub simulator: Option<odyssey_sim::SensorModel>,
    /// Leave USB devices and the system Bluetooth radio alone.
    #[cfg(feature = "sim")]
    pub no_discovery: bool,
}

/// Runs the hub: the gRPC server on the local socket, the device tasks, the accessory
/// manager and the forwarding of device task messages into the shared lists.
///
/// Returns once the gRPC server or the device tasks end, or `cancel_token` fires.
/// Critical task failures are returned rather than reported as a clean shutdown.
/// Cancellation does not wait for clients to close their streaming RPCs; Bluetooth
/// readers are stopped and the manager disconnects its accessories before returning.
#[cfg_attr(
    not(feature = "sim"),
    expect(
        unused_variables,
        reason = "the options only have fields in simulator builds"
    )
)]
pub async fn run_server(
    sender: mpsc::UnboundedSender<Message>,
    cancel_token: CancellationToken,
    options: ServerOptions,
) -> Result<(), ServerError> {
    let (name, connections) = bind_local_socket(&sender).await?;

    let (device_sender, device_messages) = mpsc::channel(DEVICE_MESSAGE_CAPACITY);
    let (event_sender, _) = broadcast::channel(EVENT_BROADCAST_CAPACITY);
    let (device_list_change_sender, _) = broadcast::channel::<()>(DEVICE_LIST_CHANGE_CAPACITY);
    let (dongle_list_change_sender, _) = broadcast::channel::<()>(DONGLE_LIST_CHANGE_CAPACITY);
    let (accessory_map_sender, _) = broadcast::channel(ACCESSORY_MAP_CAPACITY);

    // Both background tasks also stop if the run_server future itself is dropped.
    let background_cancel = cancel_token.child_token();
    let _background_stop = background_cancel.clone().drop_guard();
    let config = StartupConfig::load(background_cancel.clone()).await;
    #[cfg(feature = "sim")]
    let simulator = options
        .simulator
        .map(odyssey_sim::Simulator::with_sensor_model);
    #[cfg(feature = "sim")]
    let simulator_control = match simulator.as_ref() {
        Some(simulator) => Some(
            crate::sim::start(
                simulator.handle(),
                Arc::clone(&config.screen_calibrations),
                background_cancel.clone(),
            )
            .await?,
        ),
        None => None,
    };

    let shot_delay_watch = Arc::new(RwLock::new(HashMap::new()));
    let (accessory_info_sender, accessory_info_receiver) =
        watch::channel(config.accessory_info.clone());
    let accessory_map = Arc::new(parking_lot::Mutex::new(
        config
            .accessory_info
            .into_iter()
            .map(|(id, info)| (id, (info, false)))
            .collect(),
    ));

    let marker_demand = crate::accessories::MarkerDemand::default();
    let marker_demand_watch = marker_demand.subscribe();
    let (accessory_radios, accessory_commands) = crate::accessories::AccessoryRadios::new();
    let server = HubServer {
        device_list: Default::default(),
        event_sender: event_sender.clone(),
        marker_demand,
        device_list_change_sender: device_list_change_sender.clone(),
        screen_calibrations: Arc::clone(&config.screen_calibrations),
        device_shot_delays: Arc::clone(&config.device_shot_delays),
        shot_delay_watch: Arc::clone(&shot_delay_watch),
        accessory_map: Arc::clone(&accessory_map),
        accessory_map_sender: accessory_map_sender.clone(),
        accessory_radios: accessory_radios.clone(),
        accessory_config_lock: Default::default(),
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
    let accessory_dongles = server.dongle_list.clone();

    if let Err(e) = sender.send(Message::ServerInit(Ok(config.corruptions))) {
        tracing::warn!("Failed to send server init message: {}", e);
    }
    tracing::info!("Server running at {name:?}");

    // Critical parts are supervised together; the optional Bluetooth manager
    // owns its connections and is given time to disconnect during shutdown.
    let accessory_cancel = background_cancel.clone();
    let accessory_task = tokio::spawn(async move {
        #[cfg(feature = "sim")]
        let native_enabled = !options.no_discovery;
        #[cfg(not(feature = "sim"))]
        let native_enabled = true;
        accessory_manager(
            event_sender,
            accessory_info_receiver,
            accessory_map_sender,
            device_list,
            accessory_map,
            marker_demand_watch,
            accessory_dongles,
            accessory_radios,
            accessory_commands,
            native_enabled,
            accessory_cancel,
        )
        .await;
    });
    let device_task_events = server.event_sender.clone();
    let device_tasks_stream = stream::once(async move {
        tracing::info!("device_tasks_stream: starting");
        let result = device_tasks(
            device_sender,
            config.screen_calibrations,
            config.device_offsets,
            config.device_shot_delays,
            shot_delay_watch,
            device_task_events,
            #[cfg(feature = "sim")]
            device_tasks::Simulation {
                simulator,
                discovery: !options.no_discovery,
            },
        )
        .await;
        ServerEvent::DeviceTasks(result)
    });

    let message_stream = stream::unfold(
        (device_messages, forwarder),
        |(mut messages, forwarder): (mpsc::Receiver<device_tasks::Message>, _)| async move {
            let message = messages.recv().await?;
            forwarder.forward(message);
            Some((ServerEvent::MessageForwarded, (messages, forwarder)))
        },
    );

    let tonic_cancel = cancel_token.clone();
    let tonic_stream = stream::once(async move {
        let result = tonic::transport::Server::builder()
            .add_service(ServiceServer::new(server))
            .serve_with_incoming_shutdown(connections, tonic_cancel.cancelled())
            .await;
        ServerEvent::TonicServer(result)
    });

    tracing::info!("run_server: merging streams");
    let merged = (device_tasks_stream, message_stream, tonic_stream).merge();
    tokio::pin!(merged);
    tracing::info!("run_server: entering main loop");

    let result = loop {
        let event = tokio::select! {
            () = cancel_token.cancelled() => break Ok(()),
            event = merged.next() => match event {
                Some(event) => event,
                None => break Ok(()),
            },
        };
        tracing::debug!("Server loop event: {:?}", event);
        match event {
            ServerEvent::DeviceTasks(result) => {
                break Err(match result {
                    Ok(()) => ServerError::DeviceTasksExited,
                    Err(never) => match never {},
                });
            }
            ServerEvent::MessageForwarded => tracing::trace!("Event forwarded"),
            ServerEvent::TonicServer(result) => {
                break result.map_err(ServerError::Transport);
            }
        }
    };
    background_cancel.cancel();
    #[cfg(feature = "sim")]
    if let Some(task) = simulator_control
        && let Err(error) = task.await
    {
        tracing::warn!("Simulator control task failed: {error}");
    }
    if let Err(error) = accessory_task.await {
        tracing::warn!("Accessory manager task failed: {error}");
    }

    tracing::info!("Server main loop ended");

    result
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
        assert_send(run_server(
            sender,
            CancellationToken::new(),
            ServerOptions::default(),
        ));
    }
}
