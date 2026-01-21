use ats_usb::packets::vm::VendorData;
use odyssey_hub_common as common;

#[macro_export]
macro_rules! impl_from_simple {
    ($from:path => $to:ty, $($field:ident),+) => {
        impl From<$from> for $to {
            fn from(value: $from) -> Self {
                Self {
                    $(
                        $field: value.$field.into(),
                    )+
                }
            }
        }
    };
}

#[derive(uniffi::Record, Clone)]
pub struct Device {
    pub uuid: Vec<u8>,
    pub transport: common::device::Transport,
    pub capabilities: u8,
    pub firmware_version: Option<Vec<u16>>,
    pub events_transport: common::device::EventsTransport,
    pub events_connected: bool,
}

#[derive(uniffi::Enum, Clone)]
pub enum Event {
    AccessoryEvent(AccessoryEvent),
    DeviceEvent(DeviceEvent),
}

#[derive(uniffi::Record, Clone)]
pub struct DeviceEvent {
    device: Device,
    kind: DeviceEventKind,
}

#[derive(uniffi::Enum, Clone)]
pub enum DeviceEventKind {
    AccelerometerEvent(AccelerometerEvent),
    TrackingEvent(TrackingEvent),
    ImpactEvent(ImpactEvent),
    ZeroResult(bool),
    SaveZeroResult(bool),
    PacketEvent(PacketEvent),
    CapabilitiesChanged,
}

#[derive(uniffi::Record, Clone)]
pub struct AccessoryEvent {
    pub info: AccessoryInfo,
    pub kind: AccessoryEventKind,
}

#[derive(uniffi::Enum, Clone)]
pub enum AccessoryEventKind {
    Connect(Option<u64>),
    Disconnect,
    AssignmentChange(Option<u64>),
}

#[derive(uniffi::Record, Clone)]
pub struct AccelerometerEvent {
    pub timestamp: u32,
    pub accel: crate::funny::Vector3f32,
    pub gyro: crate::funny::Vector3f32,
    pub euler_angles: crate::funny::Vector3f32,
}

#[derive(uniffi::Record, Clone)]
pub struct TrackingEvent {
    pub timestamp: u32,
    pub aimpoint: crate::funny::Vector2f32,
    pub pose: Pose,
    pub distance: f32,
    pub screen_id: u32,
}

#[derive(uniffi::Record, Clone)]
pub struct ImpactEvent {
    pub timestamp: u32,
}

#[derive(uniffi::Record, Clone)]
pub struct PacketEvent {
    pub ty: u8,
    pub data: PacketData,
}

#[derive(uniffi::Enum, Clone)]
pub enum PacketData {
    Unsupported,
    VendorEvent(VendorEventPacketData),
}

#[derive(uniffi::Record, Clone)]
pub struct VendorEventPacketData {
    pub len: u8,
    pub data: Vec<u8>,
}

#[derive(uniffi::Record, Clone, Default)]
pub struct Pose {
    pub rotation: crate::funny::Matrix3f32,
    pub translation: crate::funny::Matrix3x1f32,
}

#[derive(uniffi::Record, Clone)]
pub struct ScreenInfo {
    pub id: u8,
    pub tl: crate::funny::Vector2f32,
    pub tr: crate::funny::Vector2f32,
    pub bl: crate::funny::Vector2f32,
    pub br: crate::funny::Vector2f32,
}

#[derive(uniffi::Enum, Clone)]
pub enum AccessoryType {
    DryFireMag,
}

#[derive(uniffi::Record, Clone)]
pub struct AccessoryInfo {
    pub name: String,
    pub ty: AccessoryType,
    pub assignment: Option<u64>,
}

#[derive(uniffi::Record, Clone)]
pub struct AccessoryStatus {
    pub info: AccessoryInfo,
    pub connected: bool,
}

#[derive(uniffi::Record, Clone)]
pub struct AccessoryMapEntry {
    pub key: u64,
    pub status: AccessoryStatus,
}

#[derive(uniffi::Record, Clone)]
pub struct AccessoryInfoMapEntry {
    pub key: u64,
    pub info: AccessoryInfo,
}

impl_from_simple!(common::events::AccelerometerEvent => AccelerometerEvent, timestamp, accel, gyro, euler_angles);
impl_from_simple!(common::events::ImpactEvent => ImpactEvent, timestamp);
impl_from_simple!(common::events::Pose => Pose, rotation, translation);
impl_from_simple!(Pose => common::events::Pose, rotation, translation);

impl From<common::accessory::AccessoryType> for AccessoryType {
    fn from(accessory_type: common::accessory::AccessoryType) -> Self {
        match accessory_type {
            common::accessory::AccessoryType::DryFireMag => Self::DryFireMag,
            common::accessory::AccessoryType::BlackbeardX => Self::DryFireMag,
        }
    }
}

impl From<common::accessory::AccessoryInfo> for AccessoryInfo {
    fn from(info: common::accessory::AccessoryInfo) -> Self {
        AccessoryInfo {
            name: info.name,
            ty: info.ty.into(),
            assignment: info.assignment.map(|nz| nz.get()),
        }
    }
}

impl From<AccessoryType> for common::accessory::AccessoryType {
    fn from(accessory_type: AccessoryType) -> Self {
        match accessory_type {
            AccessoryType::DryFireMag => common::accessory::AccessoryType::DryFireMag,
        }
    }
}

impl From<AccessoryInfo> for common::accessory::AccessoryInfo {
    fn from(info: AccessoryInfo) -> Self {
        let mut r = common::accessory::AccessoryInfo::with_defaults(info.name, info.ty.into());
        r.assignment = info.assignment.and_then(|v| std::num::NonZeroU64::new(v));
        r
    }
}

impl From<common::ScreenInfo> for ScreenInfo {
    fn from(screen_info: common::ScreenInfo) -> Self {
        ScreenInfo {
            id: screen_info.id,
            tl: screen_info.bounds[0].into(),
            tr: screen_info.bounds[1].into(),
            bl: screen_info.bounds[2].into(),
            br: screen_info.bounds[3].into(),
        }
    }
}

impl From<common::events::TrackingEvent> for TrackingEvent {
    fn from(e: common::events::TrackingEvent) -> Self {
        Self {
            timestamp: e.timestamp,
            aimpoint: e.aimpoint.into(),
            pose: e.pose.into(),
            distance: e.distance,
            screen_id: e.screen_id,
        }
    }
}

impl From<TrackingEvent> for common::events::TrackingEvent {
    fn from(e: TrackingEvent) -> Self {
        Self {
            timestamp: e.timestamp,
            aimpoint: e.aimpoint.into(),
            pose: e.pose.into(),
            distance: e.distance,
            screen_id: e.screen_id,
        }
    }
}

impl From<common::device::Device> for Device {
    fn from(device: common::device::Device) -> Self {
        Self {
            uuid: device.uuid.into(),
            transport: device.transport,
            capabilities: device.capabilities.bits(),
            firmware_version: device.firmware_version.map(|v| v.to_vec()),
            events_transport: device.events_transport,
            events_connected: device.events_connected,
        }
    }
}

impl From<Device> for common::device::Device {
    fn from(device: Device) -> Self {
        Self {
            uuid: device.uuid.try_into().unwrap(),
            transport: device.transport,
            capabilities: common::device::DeviceCapabilities::new(device.capabilities),
            firmware_version: device.firmware_version.map(|v| v.try_into().unwrap()),
            events_transport: device.events_transport,
            events_connected: device.events_connected,
        }
    }
}

impl From<ats_usb::packets::vm::PacketData> for PacketData {
    fn from(packet_data: ats_usb::packets::vm::PacketData) -> Self {
        match packet_data {
            ats_usb::packets::vm::PacketData::Vendor(_, VendorData { len, data }) => {
                PacketData::VendorEvent(VendorEventPacketData {
                    len,
                    data: data.to_vec(),
                })
            }
            _ => PacketData::Unsupported,
        }
    }
}

impl From<common::events::Event> for Event {
    fn from(event: common::events::Event) -> Self {
        match event {
            common::events::Event::DeviceEvent(common::events::DeviceEvent(d, evt)) => {
                Event::DeviceEvent(DeviceEvent {
                    device: d.into(),
                    kind: match evt {
                        common::events::DeviceEventKind::AccelerometerEvent(e) => {
                            DeviceEventKind::AccelerometerEvent(e.into())
                        }
                        common::events::DeviceEventKind::TrackingEvent(e) => {
                            DeviceEventKind::TrackingEvent(TrackingEvent {
                                timestamp: e.timestamp,
                                aimpoint: e.aimpoint.into(),
                                pose: e.pose.into(),
                                distance: e.distance,
                                screen_id: e.screen_id,
                            })
                        }
                        common::events::DeviceEventKind::ImpactEvent(e) => {
                            DeviceEventKind::ImpactEvent(e.into())
                        }
                        common::events::DeviceEventKind::ZeroResult(b) => {
                            DeviceEventKind::ZeroResult(b)
                        }
                        common::events::DeviceEventKind::SaveZeroResult(b) => {
                            DeviceEventKind::SaveZeroResult(b)
                        }
                        common::events::DeviceEventKind::PacketEvent(p) => {
                            DeviceEventKind::PacketEvent(PacketEvent {
                                ty: p.ty().into(),
                                data: p.data.into(),
                            })
                        }
                        common::events::DeviceEventKind::CapabilitiesChanged => {
                            DeviceEventKind::CapabilitiesChanged
                        }
                    },
                })
            }
        }
    }
}
