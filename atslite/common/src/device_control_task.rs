//! Device control task - handles USB control endpoint commands
//! Replaces triple-press bond clearing with CLI-based control

use core::sync::atomic::{AtomicPtr, Ordering};

use defmt::info;
use protodongers::control::device::{DeviceMsg, PairingError, TransportMode, Version};

use crate::ble::peripheral;
use crate::device_control;
use crate::transport_mode;

/// Function pointer registered by each board to persist the transport mode to flash.
/// Call `register_transport_mode_persist_fn` once during board init.
static TRANSPORT_MODE_PERSIST_FN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Register a board-specific function that persists the transport mode to flash.
/// Must be called before `device_control_task` starts.
pub fn register_transport_mode_persist_fn(f: fn(TransportMode)) {
    TRANSPORT_MODE_PERSIST_FN.store(f as *mut (), Ordering::Release);
}

fn persist_transport_mode(mode: TransportMode) {
    let ptr = TRANSPORT_MODE_PERSIST_FN.load(Ordering::Acquire);
    if !ptr.is_null() {
        let f: fn(TransportMode) = unsafe { core::mem::transmute(ptr) };
        f(mode);
    }
}

#[embassy_executor::task]
pub async fn device_control_task(firmware_version: [u16; 3]) {
    info!("Device control task started");

    // If pairing is active, we multiplex between incoming commands and the pairing wait/timeout
    let mut pairing_deadline: Option<embassy_time::Instant> = None;

    loop {
        // When pairing is in progress, listen for both commands and pairing completion/timeout.
        if let Some(deadline) = pairing_deadline {
            use embassy_futures::select::{select3, Either3};
            let cmd_fut = device_control::recv_cmd();
            let complete_fut = peripheral::wait_for_pairing_complete();
            let timeout_fut = embassy_time::Timer::at(deadline);

            match select3(cmd_fut, complete_fut, timeout_fut).await {
                Either3::First(cmd) => {
                    info!("Received control command: {:?}", cmd);
                    // Handle commands below (falls through)
                    match cmd {
                        DeviceMsg::CancelPairing => {
                            info!("Cancelling pairing mode");
                            peripheral::cancel_pairing_mode();
                            pairing_deadline = None;
                            device_control::try_send_event(DeviceMsg::PairingResult(Err(
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
                            handle_cmd(other, &mut pairing_deadline, firmware_version).await;
                            continue;
                        }
                    }
                }
                Either3::Second(addr) => {
                    info!("Pairing completed successfully");
                    pairing_deadline = None;
                    device_control::try_send_event(DeviceMsg::PairingResult(Ok(addr)));
                    continue;
                }
                Either3::Third(_) => {
                    info!("Pairing timed out");
                    pairing_deadline = None;
                    peripheral::cancel_pairing_mode();
                    device_control::try_send_event(DeviceMsg::PairingResult(Err(
                        PairingError::Timeout,
                    )));
                    continue;
                }
            }
        }

        let cmd = device_control::recv_cmd().await;
        info!("Received control command: {:?}", cmd);
        handle_cmd(cmd, &mut pairing_deadline, firmware_version).await;
    }
}

async fn handle_cmd(
    cmd: DeviceMsg,
    pairing_deadline: &mut Option<embassy_time::Instant>,
    firmware_version: [u16; 3],
) {
    match cmd {
        DeviceMsg::ReadVersion() => {
            let version = Version::new(firmware_version);
            device_control::try_send_event(DeviceMsg::ReadVersionResponse(version));
        }

        DeviceMsg::ReadUuid() => {
            let uuid: [u8; 6] = crate::utils::device_id()[..6].try_into().unwrap();
            device_control::try_send_event(DeviceMsg::ReadUuidResponse(uuid));
        }

        DeviceMsg::ClearBond => {
            info!("Clearing bond...");
            // Clear the bond - caller must provide settings access
            // unsafe { crate::settings::get_settings() }.ble_bond_clear();

            // Request disconnect if currently connected
            peripheral::request_disconnect();

            info!("Bond cleared successfully");
            device_control::try_send_event(DeviceMsg::ClearBondResponse(Ok(())));
        }

        DeviceMsg::StartPairing(params) => {
            if transport_mode::get() != TransportMode::Ble {
                info!("StartPairing rejected: transport mode is not BLE");
                device_control::try_send_event(DeviceMsg::PairingResult(Err(
                    PairingError::NotBleMode,
                )));
                return;
            }

            info!("Starting pairing mode for {} ms", params.timeout_ms);
            // Enter pairing mode
            peripheral::enter_pairing_mode();

            // Clear any existing bond so pairing is fresh
            // unsafe { crate::settings::get_settings() }.ble_bond_clear();
            peripheral::request_disconnect();

            device_control::try_send_event(DeviceMsg::StartPairingResponse);

            // Track deadline so the main loop can keep serving commands while pairing runs
            *pairing_deadline = Some(
                embassy_time::Instant::now()
                    + embassy_time::Duration::from_millis(params.timeout_ms as u64),
            );
        }

        DeviceMsg::CancelPairing => {
            info!("Cancelling pairing mode");
            peripheral::cancel_pairing_mode();
            *pairing_deadline = None;
            device_control::try_send_event(DeviceMsg::PairingResult(Err(PairingError::Cancelled)));
        }

        DeviceMsg::GetTransportMode => {
            let mode = transport_mode::get();
            info!("Reporting transport mode: {:?}", mode);
            device_control::try_send_event(DeviceMsg::TransportModeStatus(mode));
        }

        DeviceMsg::SetTransportMode(mode) => {
            info!("Setting transport mode: {:?}", mode);
            transport_mode::set(mode);
            if matches!(mode, TransportMode::Usb) {
                // Drop any active BLE data sessions to keep transports exclusive
                peripheral::request_disconnect();
            }
            // Respond to the host BEFORE persisting so the USB control_in poll is
            // answered before the blocking flash write stalls the executor.
            device_control::try_send_event(DeviceMsg::TransportModeStatus(mode));
            // Persist mode to flash so it survives reboot
            persist_transport_mode(mode);
        }

        DeviceMsg::Reboot => {
            info!("Reboot requested, sending ack then resetting");
            device_control::try_send_event(DeviceMsg::RebootAck);
            // Small delay to allow the RebootAck to be polled by the host before reset
            embassy_time::Timer::after_millis(50).await;
            cortex_m::peripheral::SCB::sys_reset();
        }

        DeviceMsg::ReadConfig(kind) => {
            let settings = unsafe { crate::settings::get_settings() };
            let g = &settings.general;
            use protodongers::{ConfigKind, GeneralConfig};
            let config = match kind {
                ConfigKind::ImpactThreshold => GeneralConfig::ImpactThreshold(g.impact_threshold),
                ConfigKind::SuppressMs => GeneralConfig::SuppressMs(g.suppress_ms),
                ConfigKind::AccelConfig => GeneralConfig::AccelConfig(g.accel_config.clone()),
                ConfigKind::GyroConfig => GeneralConfig::GyroConfig(g.gyro_config.clone()),
                ConfigKind::CameraModelNf => GeneralConfig::CameraModelNf(g.camera_model_nf.clone().into()),
                ConfigKind::CameraModelWf => GeneralConfig::CameraModelWf(g.camera_model_wf.clone().into()),
                ConfigKind::StereoIso => GeneralConfig::StereoIso(g.stereo_iso.clone().into()),
            };
            device_control::try_send_event(DeviceMsg::ReadConfigResponse(config));
        }

        DeviceMsg::WriteConfig(config) => {
            let settings = unsafe { crate::settings::get_settings() };
            let wire_config = protodongers::wire::GeneralConfig::from(config);
            settings.general.set_general_config(&wire_config);
            device_control::try_send_event(DeviceMsg::WriteConfigAck);
        }

        DeviceMsg::FlashSettings => {
            let settings = unsafe { crate::settings::get_settings() };
            settings.general_write();
            device_control::try_send_event(DeviceMsg::FlashSettingsAck);
        }

        _ => {
            // Ignore response messages received as commands
        }
    }
}
