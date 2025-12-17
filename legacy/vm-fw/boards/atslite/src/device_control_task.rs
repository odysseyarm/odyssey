//! Device control task - handles USB control endpoint commands
//! Replaces triple-press bond clearing with CLI-based control

use defmt::info;
use protodongers::control::device::{DeviceMsg, PairingError, TransportMode, Version};

const FIRMWARE_VERSION: [u16; 3] = [0, 1, 0]; // TODO: Get from build

#[embassy_executor::task]
pub async fn device_control_task() {
    info!("Device control task started");

    // If pairing is active, we multiplex between incoming commands and the pairing wait/timeout
    let mut pairing_deadline: Option<embassy_time::Instant> = None;

    loop {
        // When pairing is in progress, listen for both commands and pairing completion/timeout.
        if let Some(deadline) = pairing_deadline {
            use embassy_futures::select::{Either3, select3};
            let cmd_fut = common::device_control::recv_cmd();
            let complete_fut = crate::ble::peripheral::wait_for_pairing_complete();
            let timeout_fut = embassy_time::Timer::at(deadline);

            match select3(cmd_fut, complete_fut, timeout_fut).await {
                Either3::First(cmd) => {
                    info!("Received control command: {:?}", cmd);
                    // Handle commands below (falls through)
                    match cmd {
                        DeviceMsg::CancelPairing => {
                            info!("Cancelling pairing mode");
                            crate::ble::peripheral::cancel_pairing_mode();
                            pairing_deadline = None;
                            common::device_control::try_send_event(DeviceMsg::PairingResult(Err(
                                PairingError::Cancelled,
                            )));
                            continue;
                        }
                        // Allow repeated StartPairing to reset the timer
                        DeviceMsg::StartPairing(params) => {
                            info!(
                                "StartPairing received during active pairing, resetting timeout to {} ms",
                                params.timeout_ms
                            );
                            pairing_deadline = Some(
                                embassy_time::Instant::now()
                                    + embassy_time::Duration::from_millis(params.timeout_ms as u64),
                            );
                            continue;
                        }
                        other => {
                            // Process other commands normally
                            handle_cmd(other, &mut pairing_deadline).await;
                            continue;
                        }
                    }
                }
                Either3::Second(addr) => {
                    info!("Pairing completed successfully");
                    pairing_deadline = None;
                    common::device_control::try_send_event(DeviceMsg::PairingResult(Ok(addr)));
                    continue;
                }
                Either3::Third(_) => {
                    info!("Pairing timed out");
                    pairing_deadline = None;
                    crate::ble::peripheral::cancel_pairing_mode();
                    common::device_control::try_send_event(DeviceMsg::PairingResult(Err(
                        PairingError::Timeout,
                    )));
                    continue;
                }
            }
        }

        let cmd = common::device_control::recv_cmd().await;
        info!("Received control command: {:?}", cmd);
        handle_cmd(cmd, &mut pairing_deadline).await;
    }
}

async fn handle_cmd(
    cmd: DeviceMsg,
    pairing_deadline: &mut Option<embassy_time::Instant>,
) {
    match cmd {
        DeviceMsg::ReadVersion() => {
            let version = Version::new(FIRMWARE_VERSION);
            common::device_control::try_send_event(DeviceMsg::ReadVersionResponse(version));
        }

        DeviceMsg::ClearBond => {
            info!("Clearing bond...");
            // Clear the bond
            unsafe { common::settings::get_settings() }.ble_bond_clear();

            // Request disconnect if currently connected
            crate::ble::peripheral::request_disconnect();

            info!("Bond cleared successfully");
            common::device_control::try_send_event(DeviceMsg::ClearBondResponse(Ok(())));
        }

        DeviceMsg::StartPairing(params) => {
            if crate::transport_mode::get() != TransportMode::Ble {
                info!("StartPairing rejected: transport mode is not BLE");
                common::device_control::try_send_event(DeviceMsg::PairingResult(Err(
                    PairingError::NotBleMode,
                )));
                return;
            }

            info!("Starting pairing mode for {} ms", params.timeout_ms);
            // Enter pairing mode
            crate::ble::peripheral::enter_pairing_mode();

            // Clear any existing bond so pairing is fresh
            unsafe { common::settings::get_settings() }.ble_bond_clear();
            crate::ble::peripheral::request_disconnect();

            common::device_control::try_send_event(DeviceMsg::StartPairingResponse);

            // Track deadline so the main loop can keep serving commands while pairing runs
            *pairing_deadline = Some(
                embassy_time::Instant::now()
                    + embassy_time::Duration::from_millis(params.timeout_ms as u64),
            );
        }

        DeviceMsg::CancelPairing => {
            info!("Cancelling pairing mode");
            crate::ble::peripheral::cancel_pairing_mode();
            *pairing_deadline = None;
            common::device_control::try_send_event(DeviceMsg::PairingResult(Err(
                PairingError::Cancelled,
            )));
        }

        DeviceMsg::GetTransportMode => {
            let mode = crate::transport_mode::get();
            info!("Reporting transport mode: {:?}", mode);
            common::device_control::try_send_event(DeviceMsg::TransportModeStatus(mode));
        }

        DeviceMsg::SetTransportMode(mode) => {
            info!("Setting transport mode: {:?}", mode);
            crate::transport_mode::set(mode);
            if matches!(mode, TransportMode::Usb) {
                // Drop any active BLE data sessions to keep transports exclusive
                crate::ble::peripheral::request_disconnect();
            }
            common::device_control::try_send_event(DeviceMsg::TransportModeStatus(mode));
        }

        _ => {
            // Ignore response messages received as commands
        }
    }
}
