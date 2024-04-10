use std::{ffi::OsString, time::Duration};

use tokio::sync::mpsc;

use odyssey_hub_service::service::{run_service, Message};

#[cfg(target_os = "windows")]
use windows_service::define_windows_service;
#[cfg(target_os = "windows")]
use windows_service::{service::{ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType}, service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle}, service_dispatcher};

#[cfg(target_os = "windows")]
define_windows_service!(ffi_service_main, service_main);

#[cfg(target_os = "windows")]
pub fn start() {
    match service_dispatcher::start("Odyssey", ffi_service_main) {
        Ok(_) => (),
        Err(e) => eprintln!("Error starting service: {:?}", e),
    }
}

#[cfg(target_os = "windows")]
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
            }) => {},
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

#[cfg(target_os = "windows")]
async fn handle_service_status(
    status_handle: ServiceStatusHandle,
    mut receiver: mpsc::UnboundedReceiver<Message>,
    running_status: ServiceStatus,
) {
    use odyssey_hub_service::service::Message;

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
