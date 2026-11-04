use std::sync::Arc;

use nalgebra::{Point2, Translation3};
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::TrackingEvent;
use odyssey_usb::device::{MuxDevice, VmDevice};
use tokio::sync::{mpsc, oneshot};

/// Answer channel of a request: the value, or the reason it failed, as text for the RPC reply.
pub(crate) type Reply<T> = oneshot::Sender<Result<T, String>>;

/// Control messages sent into per-device task
#[derive(Debug)]
pub(crate) enum DeviceTaskMessage {
    ResetZero,
    SaveZero,
    /// Send a vendor packet; the USB layer rejects invalid tags or payloads over 98 bytes.
    WriteVendor(u8, Vec<u8>),
    /// Set the shot delay (milliseconds) for this device. Server may send this
    /// to apply shot delay live to the running device task.
    SetShotDelay(u16),
    Zero(Translation3<f32>, Point2<f32>, Option<TrackingEvent>),
    ClearZero,
    /// Read a sensor register. Reply channel returns the register value or an error.
    ReadRegister {
        port: u8,
        bank: u8,
        address: u8,
        reply: Reply<u8>,
    },
    /// Write a sensor register. Reply channel returns success or an error.
    WriteRegister {
        port: u8,
        bank: u8,
        address: u8,
        data: u8,
        reply: Reply<()>,
    },
    /// Read a device config value (impact threshold, suppress ms, etc.)
    ReadConfig {
        kind: u8,
        reply: Reply<u8>,
    },
    /// Write a device config value.
    WriteConfig {
        kind: u8,
        value: u8,
        reply: Reply<()>,
    },
    /// Persist current settings to device flash. Reply channel returns success or an error.
    FlashSettings {
        reply: Reply<()>,
    },
    /// Start BLE pairing mode. Reply returns Ok(()) when pairing mode is entered.
    /// The actual pairing result arrives as a PairingResult event.
    StartPairing {
        timeout_ms: u32,
        reply: Reply<()>,
    },
    /// Cancel an in-progress pairing operation.
    CancelPairing {
        reply: Reply<()>,
    },
    /// The net core release counter read back from the application. Sent by the
    /// probe that starts when the control endpoint attaches, because the
    /// application only learns it once its BLE controller has answered. Marks the
    /// reading as settled (`NETCORE_KNOWN`), including a settled 0.
    SetNetCoreRelease(u16),
    /// Clear bond(s) on the device. For ATS: clears single bond. For dongle: clears all bonds.
    ClearBond {
        reply: Reply<()>,
    },
    /// Set the transport mode (USB or BLE). The device will persist this and apply after reboot.
    SetTransportMode {
        usb_mode: bool,
        reply: Reply<bool>,
    },
    /// Set the device name. The device firmware will persist it to flash and update BLE advertising.
    SetDeviceName {
        name: String,
        reply: Reply<()>,
    },
    /// Add an events device to enable sensor streams (for merging USB direct + BLE mux)
    AddEventsDevice {
        device: VmDevice,
        protocol_version: [u16; 3],
        hub: MuxDevice,
        address: [u8; 6],
        owner: Arc<()>,
    },
    /// Add a control device to enable control operations (for merging BLE mux + USB direct BLE mode)
    AddControlDevice {
        device: VmDevice,
        dfu: bool,
        events_protocol_version: [u16; 3],
    },
    /// Remove control device when direct USB disconnects
    RemoveControlDevice,
    /// Remove events device when BLE mux disconnects
    RemoveEventsDevice(Arc<()>),
}

/// Device connection channels
pub(crate) struct DeviceChannels {
    /// Commands channel for device operations (register read/write, zero, etc.)
    pub commands: Option<mpsc::Sender<DeviceTaskMessage>>,
}

/// Commands that can be sent to a dongle (hub manager task).
pub(crate) enum DongleTaskMessage {
    StartPairing { timeout_ms: u32, reply: Reply<()> },
    CancelPairing { reply: Reply<()> },
    ClearBonds { reply: Reply<()> },
}

/// Dongle metadata (for dongle list).
#[derive(Clone, Debug)]
pub(crate) struct DongleInfo {
    pub id: String,                         // USB serial number
    pub protocol_version: [u16; 3],         // mux endpoint protocol version
    pub control_protocol_version: [u16; 3], // control endpoint protocol version
    pub firmware_version: [u16; 3],
    pub connected_devices: Vec<[u8; 6]>, // BLE addresses of connected ATS devices
    pub bonded_devices: Vec<[u8; 6]>,    // BLE addresses of bonded ATS devices
}

/// Messages emitted by device_tasks to the rest of the system.
pub(crate) enum Message {
    Connect(Device, DeviceChannels),
    Disconnect(Device),
    UpdateDevice(Device), // Update device metadata (e.g., capabilities changed)
    UpdateName([u8; 6], String), // Update device name only (UUID, new name)
    DongleConnect(DongleInfo, mpsc::Sender<DongleTaskMessage>),
    DongleUpdate(DongleInfo),
    DongleDisconnect(String), // dongle id
}
