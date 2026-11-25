//! Handles device control commands from the host over the USB control endpoint and the BLE
//! L2CAP control channel.

use defmt::info;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Timer};
use odyssey_protocol::control::AddBondResult;
use odyssey_protocol::control::device::{
    ClearBondResult, ClearBondsError, DeviceMsg, PairingError, PairingOutcome, TransportMode,
};
use odyssey_protocol::{Packet, PacketData, PropKind, Props, SetDeviceNameResult};

use crate::ble::{l2cap_bridge, peripheral};
use crate::device_control::{self, Origin};
use crate::settings;

/// Serves device control commands forever. `active_transport` names the transport currently
/// carrying reports. `GetTransportMode` and `SetTransportMode` both answer with it.
///
/// Since: Odyssey Desktop 0.7.0.
#[embassy_executor::task]
pub async fn device_control_task(
    firmware_version: [u32; 3],
    product_id: u16,
    active_transport: fn() -> TransportMode,
) {
    info!("Device control task started");

    // Set while pairing runs, so the loop can keep serving commands until it completes or expires.
    let mut pairing: Option<Pairing> = None;

    loop {
        let deadline = pairing.map(|p| p.deadline);
        let pairing_end = async move {
            let Some(deadline) = deadline else {
                return core::future::pending().await;
            };
            match select(peripheral::wait_for_pairing_complete(), Timer::at(deadline)).await {
                Either::First(outcome) => outcome,
                Either::Second(()) => PairingOutcome::Err(PairingError::Timeout),
            }
        };
        match select(device_control::recv_any_cmd(), pairing_end).await {
            Either::First((cmd, origin)) => {
                info!("Received control command: {:?}", cmd);
                handle_cmd(
                    cmd,
                    origin,
                    &mut pairing,
                    firmware_version,
                    product_id,
                    active_transport,
                )
                .await;
            }
            Either::Second(outcome) => end_pairing(&mut pairing, outcome, Origin::Usb).await,
        }
    }
}

/// An open pairing window and the host that asked for it.
#[derive(Clone, Copy)]
struct Pairing {
    deadline: Instant,
    origin: Origin,
}

/// Sends `msg` to the host that sent the command. With `flushed`, a BLE reply has left on
/// L2CAP before this returns, for commands that drop the link next.
async fn reply(origin: Origin, msg: DeviceMsg, flushed: bool) {
    match origin {
        Origin::Usb => device_control::try_send_event(msg),
        Origin::Ble { id, session } => {
            if session.is_some_and(|s| s != l2cap_bridge::session_generation()) {
                info!("BLE session changed before the device control reply, dropping it");
                return;
            }
            let pkt = Packet {
                id,
                data: PacketData::DeviceControl(msg),
            };
            if flushed {
                l2cap_bridge::send_control_flushed(pkt).await;
            } else if l2cap_bridge::channels().try_send(pkt).is_err() {
                info!("BLE control queue full, dropping the device control reply");
            }
        }
    }
}

/// Closes the pairing window and reports `outcome` to the pairing host. The report goes out before
/// leaving pairing mode, which can drop the link.
async fn end_pairing(pairing: &mut Option<Pairing>, outcome: PairingOutcome, idle_origin: Origin) {
    info!("Pairing ended: {:?}", outcome);
    // A cancel with no window open still answers whoever asked.
    let origin = pairing.take().map_or(idle_origin, |p| p.origin);
    reply(origin, DeviceMsg::PairingResult(outcome), true).await;
    peripheral::cancel_pairing_mode();
}

async fn handle_cmd(
    cmd: DeviceMsg,
    origin: Origin,
    pairing: &mut Option<Pairing>,
    firmware_version: [u32; 3],
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
                PropKind::BootloaderVersion => {
                    Props::BootloaderVersion(crate::bootloader_version::bootloader_version())
                }
            };
            reply(origin, DeviceMsg::ReadPropResponse(prop), false).await;
        }

        DeviceMsg::ClearBond() => {
            info!("Clearing every bond...");
            let initialized = settings::is_initialized();
            if initialized {
                settings::with_settings(settings::Settings::bonds_clear);
            }
            let result = if initialized {
                ClearBondResult::Ok()
            } else {
                ClearBondResult::Err(ClearBondsError::Failed)
            };
            // A BLE requester is dropped next, so its answer must leave first.
            reply(origin, DeviceMsg::ClearBondResponse(result), true).await;
            if initialized {
                // The next advertising cycle removes the old bonds from the BLE host too.
                peripheral::request_disconnect();
            }
        }

        DeviceMsg::ListBonds() => {
            reply(
                origin,
                DeviceMsg::ListBondsResponse(peripheral::bond_slots()),
                false,
            )
            .await;
        }

        DeviceMsg::RemoveBond(bd_addr) => {
            info!("RemoveBond for {:02x}", bd_addr);
            let result = if !settings::is_initialized() {
                ClearBondResult::Err(ClearBondsError::Failed)
            } else if settings::with_settings(|s| s.bond_remove(bd_addr)) {
                ClearBondResult::Ok()
            } else {
                ClearBondResult::Err(ClearBondsError::NotFound)
            };
            let removed = matches!(result, ClearBondResult::Ok());
            let drops_link = removed && peripheral::is_connected_peer(bd_addr);
            reply(origin, DeviceMsg::RemoveBondResponse(result), drops_link).await;
            if drops_link {
                peripheral::request_disconnect();
            } else if removed {
                peripheral::reload_bonds();
            }
        }

        DeviceMsg::StartPairing(params) => {
            let deadline = Instant::now() + Duration::from_millis(params.timeout_ms.into());
            // A repeated StartPairing only restarts the timeout.
            let started = Pairing {
                deadline,
                origin: origin.detached(),
            };
            if pairing.replace(started).is_some() {
                info!("Pairing timeout reset to {} ms", params.timeout_ms);
                reply(origin, DeviceMsg::StartPairingResponse(), false).await;
                return;
            }
            if settings::is_initialized() && settings::with_settings(|s| s.bonds_full()) {
                reply(origin, DeviceMsg::StartPairingResponse(), true).await;
                end_pairing(
                    pairing,
                    PairingOutcome::Err(PairingError::BondsFull),
                    origin,
                )
                .await;
                return;
            }
            info!("Starting pairing mode for {} ms", params.timeout_ms);
            match origin {
                // Entering pairing mode drops a BLE requester's link, so its answer must leave
                // first.
                Origin::Ble { .. } => {
                    reply(origin, DeviceMsg::StartPairingResponse(), true).await;
                    peripheral::enter_pairing_mode();
                }
                // A USB host pairs right after the answer, so answer once a new peer can connect.
                Origin::Usb => {
                    peripheral::enter_pairing_mode();
                    let _ = embassy_time::with_timeout(
                        Duration::from_secs(2),
                        peripheral::wait_for_pairing_advertising(),
                    )
                    .await;
                    reply(origin, DeviceMsg::StartPairingResponse(), false).await;
                }
            }
        }

        DeviceMsg::CancelPairing() => {
            end_pairing(
                pairing,
                PairingOutcome::Err(PairingError::Cancelled),
                origin,
            )
            .await;
        }

        // The transport follows the USB state and cannot be chosen. A set request only
        // learns the current one.
        DeviceMsg::GetTransportMode() | DeviceMsg::SetTransportMode(_) => {
            let mode = active_transport();
            info!("Reporting transport mode: {:?}", mode);
            reply(origin, DeviceMsg::TransportModeStatus(mode), false).await;
        }

        DeviceMsg::Reboot() => {
            info!("Reboot requested, sending ack then resetting");
            reply(origin, DeviceMsg::RebootAck(), false).await;
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
            reply(origin, DeviceMsg::ReadConfigResponse(config), false).await;
        }

        DeviceMsg::WriteConfig(config) => {
            let wire_config = odyssey_protocol::wire::GeneralConfig::from(config);
            settings::with_settings(|s| s.general.set_general_config(&wire_config));
            reply(origin, DeviceMsg::WriteConfigAck(), false).await;
        }

        DeviceMsg::FlashSettings() => {
            settings::with_settings(|s| s.general_write());
            reply(origin, DeviceMsg::FlashSettingsAck(), false).await;
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
            reply(
                origin,
                DeviceMsg::SetDeviceNameResponse(SetDeviceNameResult::Ok()),
                false,
            )
            .await;
        }

        DeviceMsg::AddBond(entry) => {
            use odyssey_protocol::control::AddBondError;
            info!("AddBond received for {:02x}", entry.bd_addr);
            let result = if !settings::is_initialized() {
                AddBondResult::Err(AddBondError::Failed)
            } else if settings::with_settings(|s| s.bond_store(entry)).is_ok() {
                // The BLE loop loads bonds from settings into the host before it advertises
                // again, so the new bond is live from the next connection on.
                peripheral::reload_bonds();
                AddBondResult::Ok()
            } else {
                AddBondResult::Err(AddBondError::Full)
            };
            reply(origin, DeviceMsg::AddBondResponse(result), false).await;
        }
        _ => {
            // Responses sent to us as commands are ignored.
        }
    }
}
