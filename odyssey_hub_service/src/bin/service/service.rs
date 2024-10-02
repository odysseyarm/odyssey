use std::{ffi::OsString, time::Duration};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use odyssey_hub_service::service::{run_service, Message};

#[cfg(target_os = "windows")]
use windows_service::define_windows_service;
#[cfg(target_os = "windows")]
use windows_service::{
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
    service_dispatcher,
};

#[cfg(target_os = "windows")]
define_windows_service!(ffi_service_main, service_main);

#[cfg(target_os = "windows")]
pub fn start() {
    if let Err(e) = service_dispatcher::start("Odyssey", ffi_service_main) {
        eprintln!("Error starting service: {:?}", e);
    }
}

#[cfg(target_os = "windows")]
#[tokio::main]
async fn service_main(_arguments: Vec<OsString>) {
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
    let status_handle = match service_control_handler::register("OdysseyService", event_handler) {
        Ok(status_handle) => status_handle,
        Err(e) => {
            eprintln!("Error registering service control handler: {:?}", e);
            return;
        }
    };

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

    let cancel_token = CancellationToken::new();
    let result = tokio::join! {
        run_service(sender, cancel_token.clone()),
        async move {
            handle_service_status(status_handle, receiver, running_status).await;
            cancel_token.cancel();
        }
    };

    // Handle the result of the join operation
    match result {
        (Ok(_), _) => (),
        (Err(e), _) => eprintln!("Error running service: {:?}", e),
    }

    // Tell the system that service has stopped.
    match status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    }) {
        Ok(_) => (),
        Err(e) => eprintln!("Error setting service status: {:?}", e),
    }
}

#[cfg(target_os = "windows")]
async fn handle_service_status(
    status_handle: ServiceStatusHandle,
    mut receiver: mpsc::UnboundedReceiver<Message>,
    running_status: ServiceStatus,
) {
    loop {
        match receiver.recv().await {
            Some(Message::ServerInit(Ok(()))) => {
                status_handle
                    .set_service_status(running_status.clone())
                    .unwrap();
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
