use std::sync::Arc;

use nalgebra::{Point2, Translation3};
use odyssey_hub_common::device::Device;
use odyssey_hub_common::events::TrackingEvent;
use odyssey_usb::device::{MuxDevice, VmDevice};
use tokio::sync::{mpsc, oneshot};

use super::transport_selection::TransportSelection;

/// Answer channel of a request: the value, or the reason it failed, as text for the RPC reply.
pub(crate) type Reply<T> = oneshot::Sender<Result<T, String>>;

/// Lease renewals from the image RPC's request stream.
pub(crate) struct ImageRenewals(pub(crate) futures::stream::BoxStream<'static, ()>);

impl std::fmt::Debug for ImageRenewals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ImageRenewals")
    }
}

/// One stored BLE bond of a unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BondInfo {
    /// Identity address of the bonded host, little-endian as on air.
    pub bd_addr: [u8; 6],
    /// That host holds the unit's BLE link right now.
    pub connected: bool,
    /// The bonded host is this computer's Bluetooth adapter.
    pub this_computer: bool,
}

/// How a system Bluetooth pairing ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SystemPairingOutcome {
    Paired,
    /// Every bond slot of the unit is in use.
    BondsFull,
    Timeout,
    Cancelled,
    NotBleMode,
}

/// Commands for one device's handler task.
#[derive(Debug)]
pub(crate) enum DeviceTaskMessage {
    ResetZero,
    /// Capture on the unit's USB events endpoint, under its handler's ownership.
    /// Each `renewals` item extends the device lease. Its end stops capture.
    StartImages {
        config: odyssey_protocol::image::ImageStreamConfig,
        renewals: ImageRenewals,
        reply: Reply<Result<super::image_stream::ImageResponseStream, tonic::Status>>,
    },
    Reboot {
        reply: Reply<()>,
    },
    SaveZero,
    /// Send a vendor packet. The USB layer rejects invalid tags and payloads over
    /// `VendorData::CAPACITY` bytes.
    WriteVendor(u8, Vec<u8>),
    /// Send one acknowledged OTA frame and return the device's raw OTA status.
    OtaExchange {
        frame: Vec<u8>,
        reply: Reply<[u8; 22]>,
    },
    /// Shot delay in milliseconds, applied live to the running handler.
    SetShotDelay(u16),
    Zero(Translation3<f32>, Point2<f32>, Option<TrackingEvent>),
    ClearZero,
    /// Read a sensor register.
    ReadRegister {
        port: u8,
        bank: u8,
        address: u8,
        reply: Reply<u8>,
    },
    /// Write a sensor register.
    WriteRegister {
        port: u8,
        bank: u8,
        address: u8,
        data: u8,
        reply: Reply<()>,
    },
    /// Read a config value (impact threshold, suppress ms).
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
    /// Persist the current settings to the device's flash.
    FlashSettings {
        reply: Reply<()>,
    },
    /// Enter BLE pairing mode. The reply comes once pairing mode is entered. The
    /// pairing outcome arrives later as a `PairingResult` event.
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
    /// The installed bootloader's version read back from the application, `[0, 0, 0]`
    /// when the bootloader predates reporting. Sent by the probe that starts with the
    /// control endpoint or, for a unit with no cable, the Bluetooth events link.
    SetBootloaderVersion([u32; 3]),
    /// List the unit's stored BLE bonds. Needs a USB control link and device protocol 0.2.2.
    ListBonds {
        reply: Reply<Vec<BondInfo>>,
    },
    /// Remove one stored BLE bond by identity address. Same requirements as `ListBonds`.
    RemoveBond {
        bd_addr: [u8; 6],
        reply: Reply<()>,
    },
    /// Put the unit in pairing mode over USB and pair this computer's Bluetooth adapter
    /// with it. The reply comes once the pairing is settled.
    PairSystemBluetooth {
        timeout_ms: u32,
        reply: Reply<SystemPairingOutcome>,
    },
    /// Clear every bond of an ATS or a dongle.
    ClearBond {
        reply: Reply<()>,
    },
    /// Set the transport mode (USB or BLE). Firmware with a manual transport persists
    /// it and applies it after a reboot. For automatic firmware the handler answers
    /// with the link in use and sends nothing to the unit.
    SetTransportMode {
        usb_mode: bool,
        reply: Reply<bool>,
    },
    /// The device firmware persists the name to flash and updates its BLE advertising.
    SetDeviceName {
        name: String,
        reply: Reply<()>,
    },
    /// Attach an events device (BLE mux) to a handler, which enables its sensor streams.
    /// For automatic firmware on USB it becomes the standby link.
    AddEventsDevice {
        device: VmDevice,
        protocol_version: [u16; 3],
        hub: MuxDevice,
        address: [u8; 6],
        owner: Arc<()>,
    },
    /// Attach a control device (direct USB, also in BLE transport mode) to a handler.
    AddControlDevice {
        device: VmDevice,
        dfu: bool,
        events_protocol_version: [u16; 3],
        transport_selection: TransportSelection,
    },
    /// The direct USB connection went away.
    RemoveControlDevice,
    /// The BLE mux connection went away.
    RemoveEventsDevice(Arc<()>),
}

/// The channels that reach a device's handler task.
pub(crate) struct DeviceChannels {
    /// `None` for a control-only device. A reconnect without one keeps the listed channel.
    pub commands: Option<mpsc::Sender<DeviceTaskMessage>>,
}

/// Commands for a dongle's hub manager task.
pub(crate) enum DongleTaskMessage {
    StartPairing {
        timeout_ms: u32,
        reply: Reply<()>,
    },
    CancelPairing {
        reply: Reply<()>,
    },
    ClearBonds {
        reply: Reply<()>,
    },
    Accessory {
        command: odyssey_protocol::accessory::Command,
        reply: Reply<odyssey_protocol::accessory::Reply>,
    },
    SubscribeAccessoryEvents {
        reply: Reply<tokio::sync::broadcast::Receiver<odyssey_protocol::accessory::Event>>,
    },
}

/// A dongle as the dongle list shows it.
#[derive(Clone, Debug)]
pub(crate) struct DongleInfo {
    /// USB serial number.
    pub id: String,
    /// Mux endpoint protocol version.
    pub protocol_version: [u16; 3],
    /// Control endpoint protocol version.
    pub control_protocol_version: [u16; 3],
    pub firmware_version: [u32; 3],
    /// BLE addresses of the connected ATS units.
    pub connected_devices: Vec<[u8; 6]>,
    /// BLE addresses of the bonded ATS units.
    pub bonded_devices: Vec<[u8; 6]>,
}

/// Messages emitted by `device_tasks` to the rest of the system.
pub(crate) enum Message {
    Connect(Device, DeviceChannels),
    Disconnect(Device),
    /// Device metadata changed (e.g. capabilities).
    UpdateDevice(Device),
    /// Only the name of the device with this UUID changed.
    UpdateName([u8; 6], String),
    DongleConnect(DongleInfo, mpsc::Sender<DongleTaskMessage>),
    DongleUpdate(DongleInfo),
    /// Carries the dongle id.
    DongleDisconnect(String),
}
