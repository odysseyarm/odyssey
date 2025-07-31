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

#[uniffi::export(Eq, Hash)]
#[derive(uniffi::Object, PartialEq, Clone, Eq, Hash)]
pub struct Device {
    pub record: DeviceRecord,
}

#[uniffi::export]
impl Device {
    #[uniffi::constructor]
    pub fn new(record: DeviceRecord) -> Self {
        Self { record }
    }

    #[uniffi::method]
    pub fn get(&self) -> DeviceRecord {
        self.record.clone()
    }
}

#[derive(uniffi::Enum, Clone, PartialEq, Eq, Hash)]
pub enum DeviceRecord {
    Udp {
        uuid: u64,
        id: u8,
        addr: String, // e.g., "192.168.0.1:1234"
    },
    Hid {
        uuid: u64,
        path: String,
    },
    Cdc {
        uuid: u64,
        path: String,
    },
}

#[uniffi::export]
impl Device {
    #[uniffi::method]
    pub fn uuid(&self) -> u64 {
        match self.record {
            DeviceRecord::Udp { uuid, .. } => uuid,
            DeviceRecord::Hid { uuid, .. } => uuid,
            DeviceRecord::Cdc { uuid, .. } => uuid,
        }
    }
}

#[derive(uniffi::Enum, Clone)]
pub enum Event {
    AccessoryEvent(AccessoryEvent),
    DeviceEvent(DeviceEvent),
}

#[derive(uniffi::Record, Clone)]
pub struct DeviceEvent {
    device: DeviceRecord,
    kind: DeviceEventKind,
}

#[derive(uniffi::Enum, Clone)]
pub enum DeviceEventKind {
    AccelerometerEvent(AccelerometerEvent),
    TrackingEvent(TrackingEvent),
    ImpactEvent(ImpactEvent),
    ConnectEvent,
    DisconnectEvent,
    ZeroResult(bool),
    SaveZeroResult(bool),
    ShotDelayChanged(u16),
    PacketEvent(PacketEvent),
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
    pub pose: Option<Pose>,
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

impl_from_simple!(common::events::AccelerometerEvent => AccelerometerEvent, timestamp, accel, gyro, euler_angles);
impl_from_simple!(common::events::ImpactEvent => ImpactEvent, timestamp);
impl_from_simple!(common::events::Pose => Pose, rotation, translation);
impl_from_simple!(Pose => common::events::Pose, rotation, translation);

impl From<common::AccessoryType> for AccessoryType {
    fn from(accessory_type: common::AccessoryType) -> Self {
        match accessory_type {
            common::AccessoryType::DryFireMag => Self::DryFireMag,
        }
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
            pose: match e.pose {
                Some(p) => Option::Some(p.into()),
                None => Option::None,
            },
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
            pose: match e.pose {
                Some(p) => Option::Some(p.into()),
                None => Option::None,
            },
            distance: e.distance,
            screen_id: e.screen_id,
        }
    }
}

impl From<ats_usb::packets::vm::PacketData> for PacketData {
    fn from(packet_data: ats_usb::packets::vm::PacketData) -> Self {
        match packet_data {
            ats_usb::packets::vm::PacketData::Vendor(_, (len, data)) => {
                PacketData::VendorEvent(VendorEventPacketData {
                    len,
                    data: data.to_vec(),
                })
            }
            _ => PacketData::Unsupported,
        }
    }
}

impl From<common::device::Device> for DeviceRecord {
    fn from(device: common::device::Device) -> Self {
        match device {
            common::device::Device::Udp(d) => DeviceRecord::Udp {
                uuid: d.uuid,
                id: d.id,
                addr: d.addr.to_string().into(),
            },
            common::device::Device::Hid(d) => DeviceRecord::Hid {
                uuid: d.uuid,
                path: d.path.into(),
            },
            common::device::Device::Cdc(d) => DeviceRecord::Cdc {
                uuid: d.uuid,
                path: d.path.into(),
            },
        }
    }
}

impl From<DeviceRecord> for common::device::Device {
    fn from(device: DeviceRecord) -> Self {
        match device {
            DeviceRecord::Udp { uuid, id, addr } => {
                common::device::Device::Udp(common::device::UdpDevice {
                    uuid,
                    id,
                    addr: addr.to_string().parse().unwrap(),
                })
            }
            DeviceRecord::Hid { path, uuid } => {
                common::device::Device::Hid(common::device::HidDevice {
                    uuid,
                    path: path.into(),
                })
            }
            DeviceRecord::Cdc { path, uuid } => {
                common::device::Device::Cdc(common::device::CdcDevice {
                    uuid,
                    path: path.into(),
                })
            }
        }
    }
}

impl From<common::events::Event> for Event {
    fn from(event: common::events::Event) -> Self {
        match event {
            common::events::Event::DeviceEvent(
                common::events::DeviceEvent(d, evt),
            ) => Event::DeviceEvent(DeviceEvent {
                device: d.into(),
                kind: match evt {
                    common::events::DeviceEventKind::AccelerometerEvent(e) => {
                        DeviceEventKind::AccelerometerEvent(e.into())
                    }
                    common::events::DeviceEventKind::TrackingEvent(e) => {
                        DeviceEventKind::TrackingEvent(TrackingEvent {
                            timestamp: e.timestamp,
                            aimpoint: e.aimpoint.into(),
                            pose: match e.pose {
                                Some(p) => Option::Some(p.into()),
                                None => Option::None,
                            },
                            distance: e.distance,
                            screen_id: e.screen_id,
                        })
                    }
                    common::events::DeviceEventKind::ImpactEvent(e) => {
                        DeviceEventKind::ImpactEvent(e.into())
                    }
                    common::events::DeviceEventKind::ConnectEvent => {
                        DeviceEventKind::ConnectEvent
                    }
                    common::events::DeviceEventKind::DisconnectEvent => {
                        DeviceEventKind::DisconnectEvent
                    }
                    common::events::DeviceEventKind::ZeroResult(b) => {
                        DeviceEventKind::ZeroResult(b)
                    }
                    common::events::DeviceEventKind::SaveZeroResult(b) => {
                        DeviceEventKind::SaveZeroResult(b)
                    }
                    common::events::DeviceEventKind::ShotDelayChangedEvent(n) => {
                        DeviceEventKind::ShotDelayChanged(n)
                    }
                    common::events::DeviceEventKind::PacketEvent(p) => {
                        DeviceEventKind::PacketEvent(PacketEvent {
                            ty: p.ty().into(),
                            data: p.data.into(),
                        })
                    }
                },
            }),
        }
    }
}
