use interoptopus::{ffi_type, pattern::option::Option, pattern::string::String};

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

#[ffi_type]
#[derive(Clone)]
pub enum Device {
    Udp(UdpDevice),
    Hid(HidDevice),
    Cdc(CdcDevice),
}

#[ffi_type]
#[derive(Clone)]
pub struct UdpDevice {
    pub id: u8,
    pub addr: String, // e.g., "192.168.0.1:1234"
    pub uuid: [u8; 6],
}

#[ffi_type]
#[derive(Clone)]
pub struct HidDevice {
    pub path: String,
    pub uuid: [u8; 6],
}

#[ffi_type]
#[derive(Clone)]
pub struct CdcDevice {
    pub path: String,
    pub uuid: [u8; 6],
}

#[ffi_type]
#[derive(Clone)]
pub struct Event {
    pub kind: EventKind,
}

#[ffi_type]
#[derive(Clone)]
pub enum EventKind {
    None,
    DeviceEvent(DeviceEvent),
}

#[ffi_type]
#[derive(Clone)]
pub struct DeviceEvent {
    pub device: Device,
    pub kind: DeviceEventKind,
}

#[ffi_type]
#[derive(Clone)]
pub enum DeviceEventKind {
    AccelerometerEvent(AccelerometerEvent),
    TrackingEvent(TrackingEvent),
    ImpactEvent(ImpactEvent),
    ConnectEvent,
    DisconnectEvent,
    ZeroResult(bool),
    SaveZeroResult(bool),
    PacketEvent(PacketEvent),
}

#[ffi_type]
#[derive(Clone)]
pub struct AccelerometerEvent {
    pub timestamp: u32,
    pub accel: crate::funny::Vector3f32,
    pub gyro: crate::funny::Vector3f32,
    pub euler_angles: crate::funny::Vector3f32,
}

#[ffi_type]
#[derive(Clone)]
pub struct TrackingEvent {
    pub timestamp: u32,
    pub aimpoint: crate::funny::Vector2f32,
    pub pose: Option<Pose>,
    pub distance: f32,
    pub screen_id: u32,
}

#[ffi_type]
#[derive(Clone)]
pub struct ImpactEvent {
    pub timestamp: u32,
}

#[ffi_type]
#[derive(Clone)]
pub struct PacketEvent {
    pub ty: u8,
    pub data: PacketData,
}

#[ffi_type]
#[derive(Clone)]
pub enum PacketData {
    Unsupported,
    VendorEvent(VendorEventPacketData),
}

#[ffi_type]
#[derive(Clone)]
pub struct VendorEventPacketData {
    pub len: u8,
    pub data: [u8; 98],
}

#[ffi_type]
#[derive(Clone, Default)]
pub struct Pose {
    pub rotation: crate::funny::Matrix3f32,
    pub translation: crate::funny::Matrix3x1f32,
}

#[ffi_type]
#[derive(Clone)]
pub struct ScreenInfo {
    pub id: u8,
    pub tl: crate::funny::Vector2f32,
    pub tr: crate::funny::Vector2f32,
    pub bl: crate::funny::Vector2f32,
    pub br: crate::funny::Vector2f32,
}

impl_from_simple!(odyssey_hub_common::events::AccelerometerEvent => AccelerometerEvent, timestamp, accel, gyro, euler_angles);
impl_from_simple!(odyssey_hub_common::events::ImpactEvent => ImpactEvent, timestamp);
impl_from_simple!(odyssey_hub_common::events::Pose => Pose, rotation, translation);

impl From<odyssey_hub_common::ScreenInfo> for ScreenInfo {
    fn from(screen_info: odyssey_hub_common::ScreenInfo) -> Self {
        ScreenInfo {
            id: screen_info.id,
            tl: screen_info.bounds[0].into(),
            tr: screen_info.bounds[1].into(),
            bl: screen_info.bounds[2].into(),
            br: screen_info.bounds[3].into(),
        }
    }
}

impl From<odyssey_hub_common::events::TrackingEvent> for TrackingEvent {
    fn from(e: odyssey_hub_common::events::TrackingEvent) -> Self {
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
            ats_usb::packets::vm::PacketData::Vendor(_, (len, data)) => PacketData::VendorEvent(
                VendorEventPacketData {
                    len,
                    data,
                }
            ),
            _ => PacketData::Unsupported,
        }
    }
}

impl From<odyssey_hub_common::device::Device> for Device {
    fn from(device: odyssey_hub_common::device::Device) -> Self {
        match device {
            odyssey_hub_common::device::Device::Udp(d) => Device::Udp(UdpDevice {
                id: d.id,
                addr: d.addr.to_string().into(),
                uuid: d.uuid,
            }),
            odyssey_hub_common::device::Device::Hid(d) => Device::Hid(HidDevice {
                path: d.path.into(),
                uuid: d.uuid,
            }),
            odyssey_hub_common::device::Device::Cdc(d) => Device::Cdc(CdcDevice {
                path: d.path.into(),
                uuid: d.uuid,
            }),
        }
    }
}

impl From<Device> for odyssey_hub_common::device::Device {
    fn from(device: Device) -> Self {
        match device {
            Device::Udp(d) => odyssey_hub_common::device::Device::Udp(
                odyssey_hub_common::device::UdpDevice {
                    id: d.id,
                    addr: d.addr.into_string().parse().unwrap(),
                    uuid: d.uuid,
                },
            ),
            Device::Hid(d) => odyssey_hub_common::device::Device::Hid(
                odyssey_hub_common::device::HidDevice {
                    path: d.path.into(),
                    uuid: d.uuid,
                },
            ),
            Device::Cdc(d) => odyssey_hub_common::device::Device::Cdc(
                odyssey_hub_common::device::CdcDevice {
                    path: d.path.into(),
                    uuid: d.uuid,
                },
            ),
        }
    }
}

impl From<odyssey_hub_common::events::Event> for Event {
    fn from(event: odyssey_hub_common::events::Event) -> Self {
        match event {
            odyssey_hub_common::events::Event::None => Event {
                kind: EventKind::None,
            },
            odyssey_hub_common::events::Event::DeviceEvent(de) => Event {
                kind: EventKind::DeviceEvent(DeviceEvent {
                    device: de.device.into(),
                    kind: match de.kind {
                        odyssey_hub_common::events::DeviceEventKind::AccelerometerEvent(e) => {
                            DeviceEventKind::AccelerometerEvent(e.into())
                        }
                        odyssey_hub_common::events::DeviceEventKind::TrackingEvent(e) => {
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
                        odyssey_hub_common::events::DeviceEventKind::ImpactEvent(e) => {
                            DeviceEventKind::ImpactEvent(e.into())
                        }
                        odyssey_hub_common::events::DeviceEventKind::ConnectEvent => {
                            DeviceEventKind::ConnectEvent
                        }
                        odyssey_hub_common::events::DeviceEventKind::DisconnectEvent => {
                            DeviceEventKind::DisconnectEvent
                        }
                        odyssey_hub_common::events::DeviceEventKind::ZeroResult(b) => {
                            DeviceEventKind::ZeroResult(b)
                        }
                        odyssey_hub_common::events::DeviceEventKind::SaveZeroResult(b) => {
                            DeviceEventKind::SaveZeroResult(b)
                        }
                        odyssey_hub_common::events::DeviceEventKind::PacketEvent(p) => {
                            DeviceEventKind::PacketEvent(PacketEvent {
                                ty: p.ty().into(),
                                data: p.data.into(),
                            })
                        }
                    },
                }),
            },
        }
    }
}
