//! Units reached over the host's own Bluetooth adapter (Linux): bonded to it, in reach, and
//! not already served through a cable or a dongle.
//!
//! One radio serves a unit at a time. USB wins, then a dongle, then this adapter: a unit
//! with a handler is left alone, and the unit accepts a single BLE link, so the adapter
//! cannot hold a second one next to a dongle's.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use eros::Context as _;
use odyssey_hub_common::device::{Device, DeviceCapabilities, EventsTransport, Transport};
use odyssey_hub_common::firmware_version::FirmwareVersion;
use odyssey_protocol::{PropKind, Props, Version};
use odyssey_usb::device::VmDevice;
use odyssey_usb::packets::vm::PacketData;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::UsbDeviceManager;
use crate::device_tasks::context::{
    DeviceAddress, DeviceHandles, DeviceTaskContext, RunningDevice,
};
use crate::device_tasks::handler::{DeviceConnections, DeviceHandler, DeviceHandlerInit};
use crate::device_tasks::messages::{DeviceChannels, DeviceTaskMessage, Message};
use crate::device_tasks::native_ble::BondedUnits;
use crate::device_tasks::netcore_probe::spawn_packet_probes;
use crate::device_tasks::semver::semver_at_least;
use crate::device_tasks::transport_selection::TransportSelection;

/// Capacity of a handler's control channel.
const HANDLER_CHANNEL_CAPACITY: usize = 32;

/// How long a unit is left alone after its link ends or fails to open, so a unit that
/// advertises but refuses the link is not retried on every scan.
const RELINK_BACKOFF: Duration = Duration::from_secs(10);

/// The first packet a fresh link carries is sometimes lost, so the version read is re-sent.
const VERSION_READ_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_READ_ATTEMPTS: usize = 3;

/// First device protocol that carries pairing, bond and mode control as `DeviceControl`
/// packets, so the unit needs no USB cable for them.
const DEVICE_CONTROL_PROTOCOL: [u16; 3] = [0, 3, 0];

/// What the manager remembers between scans.
#[derive(Default)]
pub(super) struct SystemBluetoothUnits {
    /// `None` until `BlueZ` answered, and again after it stopped answering.
    bonded: Option<BondedUnits>,
    /// One task per unit that has, or is opening, a link. A unit stays here through its
    /// back-off, which is what stops a second link to it from being opened.
    links: HashMap<DeviceAddress, tokio::task::JoinHandle<()>>,
}

impl UsbDeviceManager {
    /// Open a link to every bonded unit that is in reach and has no handler, and let each
    /// link's own task register it. Never waits for a link, so the USB scan stays prompt.
    pub(super) async fn scan_system_bluetooth(&mut self) {
        let state = &mut self.system_bluetooth;
        state.links.retain(|_, task| !task.is_finished());
        if state.bonded.is_none() {
            match BondedUnits::connect().await {
                Ok(bonded) => state.bonded = Some(bonded),
                Err(e) => {
                    debug!("System Bluetooth unavailable: {e}");
                    return;
                }
            }
        }
        let Some(bonded) = state.bonded.as_mut() else {
            return;
        };
        let in_reach = match bonded.in_reach().await {
            Ok(units) => units,
            Err(e) => {
                debug!("Listing bonded Bluetooth units failed: {e}");
                state.bonded = None;
                return;
            }
        };
        for address in in_reach {
            if state.links.contains_key(&address)
                || handler_running(&self.device_handles, address).await
            {
                continue;
            }
            info!("Bonded unit {address:02x?} is in reach of the system adapter");
            state.links.insert(
                address,
                tokio::spawn(serve_unit(
                    address,
                    self.context.clone(),
                    Arc::clone(&self.device_handles),
                )),
            );
        }
    }
}

/// A unit's BLE address is its uuid made static random, so a handler whose uuid maps to
/// that address serves the unit.
async fn handler_running(handles: &DeviceHandles, address: DeviceAddress) -> bool {
    handles.lock().await.iter().any(|(uuid, device)| {
        odyssey_protocol::control::ble_address(*uuid) == address && !device.task.is_finished()
    })
}

/// What a registered link needs to be watched until it ends.
struct Registered {
    vm_device: VmDevice,
    commands: mpsc::Sender<DeviceTaskMessage>,
}

/// Open, register and watch one unit's link, then wait out the back-off.
async fn serve_unit(address: DeviceAddress, context: DeviceTaskContext, handles: DeviceHandles) {
    match register(address, &context, &handles).await {
        Ok(Some(Registered {
            vm_device,
            commands,
        })) => {
            tokio::select! {
                () = vm_device.closed() => {
                    info!("System Bluetooth link to {address:02x?} closed");
                    // The handler took no owner for this link, so any owner detaches it.
                    let _ = commands
                        .send(DeviceTaskMessage::RemoveEventsDevice(Arc::new(())))
                        .await;
                }
                () = commands.closed() => {}
            }
        }
        Ok(None) => debug!("{address:02x?} is served by another link, leaving it"),
        Err(e) => warn!("System Bluetooth link to {address:02x?} failed: {e}"),
    }
    tokio::time::sleep(RELINK_BACKOFF).await;
}

/// Connect to the unit and give it a handler whose events device is the new link. `None`
/// when a handler already serves the unit (its uuid can differ from the address it
/// advertises with), in which case the link is dropped again.
async fn register(
    address: DeviceAddress,
    context: &DeviceTaskContext,
    handles: &DeviceHandles,
) -> eros::Result<Option<Registered>> {
    let vm_device = VmDevice::connect_via_l2cap(address)
        .await
        .context("Opening the L2CAP channels failed")?;
    let version = read_version(&vm_device)
        .await
        .context("Reading the firmware version failed")?;
    let selection = TransportSelection::for_protocol(version.protocol_semver);
    // Firmware that picks its own transport shares one set of streams between its links, so
    // only the older firmware has leftovers from a previous session to clear.
    if selection == TransportSelection::Manual {
        vm_device
            .clear_all_streams()
            .await
            .context("Clearing the streams of a previous session failed")?;
    }

    let uuid = match vm_device.read_prop(PropKind::Uuid).await {
        Ok(Props::Uuid(uuid)) => uuid,
        _ => address,
    };
    let product_id = match vm_device.read_prop(PropKind::ProductId).await {
        Ok(Props::ProductId(id)) => id,
        _ => 0,
    };
    let name = device_name(&vm_device, address, context).await;

    let mut capabilities = DeviceCapabilities::EVENTS;
    if selection == TransportSelection::Automatic {
        capabilities.insert(DeviceCapabilities::AUTO_TRANSPORT);
    }
    let packet_control = semver_at_least(version.protocol_semver, DEVICE_CONTROL_PROTOCOL);
    if packet_control {
        capabilities.insert(DeviceCapabilities::CONTROL);
    }
    // No `Transport` value names the system adapter: the units' OTA and device-list
    // handling for a Bluetooth-only unit keys on `UsbMux`.
    let device = Device {
        uuid,
        transport: Transport::UsbMux,
        capabilities,
        firmware_identity: FirmwareVersion::default(),
        firmware_version: version.firmware_semver,
        events_transport: EventsTransport::Bluetooth,
        events_connected: true,
        product_id,
        name,
        netcore_release: 0,
        bootloader_version: [0, 0, 0],
    };

    let (commands, control_rx) = mpsc::channel(HANDLER_CHANNEL_CAPACITY);
    let mut running = handles.lock().await;
    if running
        .get(&uuid)
        .is_some_and(|existing| !existing.task.is_finished())
    {
        return Ok(None);
    }
    let task = DeviceHandler::spawn(DeviceHandlerInit {
        connections: DeviceConnections {
            // The link is both: device control rides the packet link for units that carry it.
            control_device: packet_control.then(|| vm_device.clone()),
            events_device: Some(vm_device.clone()),
        },
        device: device.clone(),
        control_rx,
        context: context.clone(),
        protocol_version: version.protocol_semver,
        hub_device: None,
        self_tx: commands.downgrade(),
    });
    running.insert(
        uuid,
        RunningDevice {
            commands: commands.clone(),
            task,
        },
    );
    drop(running);
    context.direct_usb_handler_uuids.lock().await.remove(&uuid);
    context
        .send_message(Message::Connect(
            device,
            DeviceChannels {
                commands: Some(commands.clone()),
            },
        ))
        .await;
    // No cable, so the net core and the bootloader can only be read over this link.
    spawn_packet_probes(&commands.downgrade(), &vm_device, version.protocol_semver);
    Ok(Some(Registered {
        vm_device,
        commands,
    }))
}

async fn read_version(vm_device: &VmDevice) -> odyssey_usb::Result<Version> {
    match vm_device
        .request_with_retry(
            PacketData::ReadVersion(),
            VERSION_READ_ATTEMPT_TIMEOUT,
            VERSION_READ_ATTEMPTS,
        )
        .await?
    {
        PacketData::ReadVersionResponse(version) => Ok(version),
        other => Err(odyssey_usb::Error::UnexpectedReply(format!("{other:?}"))),
    }
}

/// The name the unit reports, else the one remembered for its address.
async fn device_name(
    vm_device: &VmDevice,
    address: DeviceAddress,
    context: &DeviceTaskContext,
) -> [u8; 33] {
    if let Ok(Props::Name(name)) = vm_device.read_prop(PropKind::Name).await
        && !name.is_empty()
    {
        let name = name.to_string();
        context
            .bond_names
            .lock()
            .await
            .insert(address, name.clone());
        return Device::name_bytes(&name);
    }
    let remembered = context
        .bond_names
        .lock()
        .await
        .get(&address)
        .cloned()
        .unwrap_or_default();
    Device::name_bytes(&remembered)
}
