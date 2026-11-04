//! Handles device control commands from the host CLI over the USB control endpoint.

use defmt::info;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Timer};
use odyssey_protocol::control::AddBondResult;
use odyssey_protocol::control::device::{
    ClearBondResult, DeviceMsg, PairingError, PairingOutcome, TransportMode,
};
use odyssey_protocol::{PropKind, Props, SetDeviceNameResult};

use crate::ble::peripheral;
use crate::{device_control, settings};

/// Serves device control commands forever. `active_transport` names the transport currently
/// carrying reports; `GetTransportMode` and `SetTransportMode` both answer with it.
#[embassy_executor::task]
pub async fn device_control_task(
    firmware_version: [u16; 3],
    product_id: u16,
    active_transport: fn() -> TransportMode,
) {
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
                handle_cmd(
                    cmd,
                    &mut pairing_deadline,
                    firmware_version,
                    product_id,
                    active_transport,
                )
                .await;
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
    product_id: u16,
    active_transport: fn() -> TransportMode,
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
                        settings::with_settings(|s| s.general.name.clone())
                    } else {
                        heapless::String::new()
                    };
                    Props::Name(name.into())
                }
                PropKind::ProductId => Props::ProductId(product_id),
                PropKind::NetCoreRelease => {
                    Props::NetCoreRelease(crate::ble::netcore::netcore_release())
                }
            };
            device_control::try_send_event(DeviceMsg::ReadPropResponse(prop));
        }

        DeviceMsg::ClearBond() => {
            info!("Clearing bond...");
            let result = if settings::is_initialized() {
                settings::with_settings(|s| s.ble_bond_clear());
                // The next advertising cycle removes the old bond from the BLE host too.
                peripheral::request_disconnect();
                ClearBondResult::Ok()
            } else {
                ClearBondResult::Err(odyssey_protocol::control::device::ClearBondsError::Failed)
            };
            device_control::try_send_event(DeviceMsg::ClearBondResponse(result));
        }

        DeviceMsg::StartPairing(params) => {
            let deadline = Instant::now() + Duration::from_millis(params.timeout_ms.into());
            // A repeated StartPairing only restarts the timeout.
            if pairing_deadline.replace(deadline).is_some() {
                info!("Pairing timeout reset to {} ms", params.timeout_ms);
                device_control::try_send_event(DeviceMsg::StartPairingResponse());
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

        // The transport follows the USB state and cannot be chosen; a set request only
        // learns the current one.
        DeviceMsg::GetTransportMode() | DeviceMsg::SetTransportMode(_) => {
            let mode = active_transport();
            info!("Reporting transport mode: {:?}", mode);
            device_control::try_send_event(DeviceMsg::TransportModeStatus(mode));
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
            let config = settings::with_settings(|settings| {
                let g = &settings.general;
                use odyssey_protocol::{ConfigKind, GeneralConfig};
                match kind {
                    ConfigKind::ImpactThreshold => {
                        GeneralConfig::ImpactThreshold(g.impact_threshold)
                    }
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
                }
            });
            device_control::try_send_event(DeviceMsg::ReadConfigResponse(config));
        }

        DeviceMsg::WriteConfig(config) => {
            let wire_config = odyssey_protocol::wire::GeneralConfig::from(config);
            settings::with_settings(|s| s.general.set_general_config(&wire_config));
            device_control::try_send_event(DeviceMsg::WriteConfigAck());
        }

        DeviceMsg::FlashSettings() => {
            settings::with_settings(|s| s.general_write());
            device_control::try_send_event(DeviceMsg::FlashSettingsAck());
        }

        DeviceMsg::SetDeviceName(name) => {
            info!("SetDeviceName: {:?}", name.as_str());
            if settings::is_initialized() {
                settings::with_settings(|s| {
                    s.general.name = name.clone().into();
                    s.general_write();
                });
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
                settings::with_settings(|s| {
                    s.ble_bond = entry.into();
                    s.ble_bond_write();
                });
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
