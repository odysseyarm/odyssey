//! Handles device control commands from the host CLI over the USB control endpoint.

use core::sync::atomic::{AtomicPtr, Ordering};

use defmt::info;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Timer};
use odyssey_protocol::control::AddBondResult;
use odyssey_protocol::control::device::{
    ClearBondResult, DeviceMsg, PairingError, PairingOutcome, TransportMode,
};
use odyssey_protocol::{PropKind, Props, SetDeviceNameResult};

use crate::ble::peripheral;
use crate::{device_control, settings, transport_mode};

/// Board-specific function that writes the transport mode to flash. Null until
/// `register_transport_mode_persist_fn` runs.
static TRANSPORT_MODE_PERSIST_FN: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Registers the function that writes the transport mode to flash. Call it during board init,
/// before `device_control_task` starts; until then mode changes are not persisted.
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

    // Set while pairing runs, so the loop can keep serving commands until it completes or expires.
    let mut pairing_deadline: Option<Instant> = None;

    loop {
        let deadline = pairing_deadline;
        let pairing_end = async move {
            let Some(deadline) = deadline else {
                return core::future::pending().await;
            };
            match select(peripheral::wait_for_pairing_complete(), Timer::at(deadline)).await {
                Either::First(device) => PairingOutcome::Ok(device),
                Either::Second(()) => PairingOutcome::Err(PairingError::Timeout),
            }
        };
        match select(device_control::recv_cmd(), pairing_end).await {
            Either::First(cmd) => {
                info!("Received control command: {:?}", cmd);
                handle_cmd(cmd, &mut pairing_deadline, firmware_version).await;
            }
            Either::Second(outcome) => end_pairing(&mut pairing_deadline, outcome),
        }
    }
}

/// Closes the pairing window and reports `outcome` to the host.
fn end_pairing(pairing_deadline: &mut Option<Instant>, outcome: PairingOutcome) {
    info!("Pairing ended: {:?}", outcome);
    *pairing_deadline = None;
    peripheral::cancel_pairing_mode();
    device_control::try_send_event(DeviceMsg::PairingResult(outcome));
}

async fn handle_cmd(
    cmd: DeviceMsg,
    pairing_deadline: &mut Option<Instant>,
    firmware_version: [u16; 3],
) {
    match cmd {
        DeviceMsg::ReadProp(kind) => {
            let prop = match kind {
                PropKind::Version => {
                    Props::Version(odyssey_protocol::Version::new(firmware_version))
                }
                PropKind::Uuid => {
                    let uuid: [u8; 6] = crate::utils::device_id()[..6].try_into().unwrap();
                    Props::Uuid(uuid)
                }
                PropKind::Name => {
                    let name = if settings::is_initialized() {
                        unsafe { settings::get_settings() }.general.name.clone()
                    } else {
                        heapless::String::new()
                    };
                    Props::Name(name.into())
                }
                PropKind::ProductId => return,
                PropKind::NetCoreRelease => {
                    Props::NetCoreRelease(crate::ble::netcore::netcore_release())
                }
            };
            device_control::try_send_event(DeviceMsg::ReadPropResponse(prop));
        }

        DeviceMsg::ClearBond() => {
            info!("Clearing bond...");
            // The bond itself is not cleared here; this only drops the current connection.

            peripheral::request_disconnect();

            info!("Bond cleared successfully");
            device_control::try_send_event(DeviceMsg::ClearBondResponse(ClearBondResult::Ok()));
        }

        DeviceMsg::StartPairing(params) => {
            if transport_mode::get() != TransportMode::Ble {
                info!("StartPairing rejected: transport mode is not BLE");
                device_control::try_send_event(DeviceMsg::PairingResult(PairingOutcome::Err(
                    PairingError::NotBleMode,
                )));
                return;
            }

            let deadline = Instant::now() + Duration::from_millis(params.timeout_ms.into());
            // A repeated StartPairing only restarts the timeout.
            if pairing_deadline.replace(deadline).is_some() {
                info!("Pairing timeout reset to {} ms", params.timeout_ms);
                return;
            }
            info!("Starting pairing mode for {} ms", params.timeout_ms);
            peripheral::enter_pairing_mode();
            device_control::try_send_event(DeviceMsg::StartPairingResponse());
        }

        DeviceMsg::CancelPairing() => {
            end_pairing(
                pairing_deadline,
                PairingOutcome::Err(PairingError::Cancelled),
            );
        }

        DeviceMsg::GetTransportMode() => {
            let mode = transport_mode::get();
            info!("Reporting transport mode: {:?}", mode);
            device_control::try_send_event(DeviceMsg::TransportModeStatus(mode));
        }

        DeviceMsg::SetTransportMode(mode) => {
            info!("Setting transport mode: {:?}", mode);
            transport_mode::set(mode);
            if matches!(mode, TransportMode::Usb) {
                // USB and BLE are exclusive, so drop any BLE session.
                peripheral::request_disconnect();
            }
            // Answer before persisting: the flash write blocks the executor, and the host's
            // control_in poll must see the reply first.
            device_control::try_send_event(DeviceMsg::TransportModeStatus(mode));
            persist_transport_mode(mode);
        }

        DeviceMsg::Reboot() => {
            info!("Reboot requested, sending ack then resetting");
            device_control::try_send_event(DeviceMsg::RebootAck());
            // Give the host time to poll the RebootAck.
            embassy_time::Timer::after_millis(50).await;
            // The host reboots straight after changing a setting, and the
            // settings worker writes asynchronously. Resetting through a page
            // erase loses the change and can leave the node undecodable, which
            // costs the device its next boot.
            settings::settings_wait_settled(embassy_time::Duration::from_millis(500)).await;
            cortex_m::peripheral::SCB::sys_reset();
        }

        DeviceMsg::ReadConfig(kind) => {
            let settings = unsafe { crate::settings::get_settings() };
            let g = &settings.general;
            use odyssey_protocol::{ConfigKind, GeneralConfig};
            let config = match kind {
                ConfigKind::ImpactThreshold => GeneralConfig::ImpactThreshold(g.impact_threshold),
                ConfigKind::SuppressMs => GeneralConfig::SuppressMs(g.suppress_ms),
                ConfigKind::AccelConfig => GeneralConfig::AccelConfig(g.accel_config.clone()),
                ConfigKind::GyroConfig => GeneralConfig::GyroConfig(g.gyro_config.clone()),
                ConfigKind::CameraModelNf => {
                    GeneralConfig::CameraModelNf(g.camera_model_nf.clone().into())
                }
                ConfigKind::CameraModelWf => {
                    GeneralConfig::CameraModelWf(g.camera_model_wf.clone().into())
                }
                ConfigKind::StereoIso => GeneralConfig::StereoIso(g.stereo_iso.clone().into()),
            };
            device_control::try_send_event(DeviceMsg::ReadConfigResponse(config));
        }

        DeviceMsg::WriteConfig(config) => {
            let settings = unsafe { crate::settings::get_settings() };
            let wire_config = odyssey_protocol::wire::GeneralConfig::from(config);
            settings.general.set_general_config(&wire_config);
            device_control::try_send_event(DeviceMsg::WriteConfigAck());
        }

        DeviceMsg::FlashSettings() => {
            let settings = unsafe { crate::settings::get_settings() };
            settings.general_write();
            device_control::try_send_event(DeviceMsg::FlashSettingsAck());
        }

        DeviceMsg::SetDeviceName(name) => {
            info!("SetDeviceName: {:?}", name.as_str());
            if settings::is_initialized() {
                let s = unsafe { settings::get_settings() };
                s.general.name = name.clone().into();
                s.general_write();
            }
            // No ADV_RESTART_SIGNAL: the dongle may be connected, and a restart would drop it.
            peripheral::set_device_name_initial(name.as_str());
            device_control::try_send_event(DeviceMsg::SetDeviceNameResponse(
                SetDeviceNameResult::Ok(),
            ));
        }

        DeviceMsg::AddBond(entry) => {
            use odyssey_protocol::control::AddBondError;
            info!("AddBond received for {:02x}", entry.bd_addr);
            if settings::is_initialized() {
                let s = unsafe { settings::get_settings() };
                s.ble_bond = entry.into();
                s.ble_bond_write();
                // The BLE loop loads the bond from settings into the host before it advertises
                // again, so the new bond is live from the next connection on.
                peripheral::request_disconnect();
                device_control::try_send_event(DeviceMsg::AddBondResponse(AddBondResult::Ok()));
            } else {
                device_control::try_send_event(DeviceMsg::AddBondResponse(AddBondResult::Err(
                    AddBondError::Failed,
                )));
            }
        }
        _ => {
            // Responses sent to us as commands are ignored.
        }
    }
}
