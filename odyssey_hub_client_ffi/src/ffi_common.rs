use std::ffi::c_char;

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

#[repr(C)]
#[derive(Clone, Copy)]
pub enum DeviceKind {
    Udp,
    Hid,
    Cdc,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UdpDevice {
    pub id: u8,
    pub addr: *const c_char, // C string
    pub uuid: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HidDevice {
    pub path: *const c_char, // C string
    pub uuid: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CdcDevice {
    pub path: *const c_char, // C string
    pub uuid: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Device {
    pub kind: DeviceKind,
    pub udp: UdpDevice,
    pub hid: HidDevice,
    pub cdc: CdcDevice,
}

impl Device {
    fn uuid(self) -> u64 {
        odyssey_hub_common::device::Device::from(self).uuid()
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Event {
    pub kind: EventKind,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub enum EventKind {
    DeviceEvent(DeviceEvent),
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DeviceEvent {
    pub device: Device,
    pub kind: DeviceEventKind,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub enum DeviceEventKind {
    AccelerometerEvent(AccelerometerEvent),
    TrackingEvent(TrackingEvent),
    ImpactEvent(ImpactEvent),
    ConnectEvent,
    DisconnectEvent,
    ZeroResult(bool),
    SaveZeroResult(bool),
    ShotDelayChanged(u8),
    PacketEvent(PacketEvent),
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AccelerometerEvent {
    pub timestamp: u32,
    pub accel: crate::funny::Vector3f32,
    pub gyro: crate::funny::Vector3f32,
    pub euler_angles: crate::funny::Vector3f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TrackingEvent {
    pub timestamp: u32,
    pub aimpoint: crate::funny::Vector2f32,
    pub pose: Pose,
    pub has_pose: bool,
    pub distance: f32,
    pub screen_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ImpactEvent {
    pub timestamp: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PacketEvent {
    pub ty: u8,
    pub data: PacketData,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub enum PacketData {
    Unsupported,
    VendorEvent(VendorEventPacketData),
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VendorEventPacketData {
    pub len: u8,
    pub data: [u8; 98],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Pose {
    pub rotation: crate::funny::Matrix3f32,
    pub translation: crate::funny::Matrix3x1f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
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
impl_from_simple!(Pose => odyssey_hub_common::events::Pose, rotation, translation);

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

impl From<ats_usb::packets::vm::PacketData> for PacketData {
    fn from(packet_data: ats_usb::packets::vm::PacketData) -> Self {
        match packet_data {
            ats_usb::packets::vm::PacketData::Vendor(_, (len, data)) => {
                PacketData::VendorEvent(VendorEventPacketData {
                    len,
                    data: {
                        let mut arr = [0u8; 98];
                        arr[..data.len()].copy_from_slice(&data);
                        arr
                    },
                })
            }
            _ => PacketData::Unsupported,
        }
    }
}

impl From<odyssey_hub_common::device::Device> for Device {
    fn from(device: odyssey_hub_common::device::Device) -> Self {
        match device {
            odyssey_hub_common::device::Device::Udp(d) => Device {
                kind: DeviceKind::Udp,
                udp: UdpDevice {
                    id: d.id,
                    addr: std::ffi::CString::new(d.addr.to_string())
                        .unwrap()
                        .into_raw(),
                    uuid: d.uuid,
                },
                hid: HidDevice {
                    path: std::ptr::null(),
                    uuid: 0,
                },
                cdc: CdcDevice {
                    path: std::ptr::null(),
                    uuid: 0,
                },
            },
            odyssey_hub_common::device::Device::Hid(d) => Device {
                kind: DeviceKind::Hid,
                udp: UdpDevice {
                    id: 0,
                    addr: std::ptr::null(),
                    uuid: 0,
                },
                hid: HidDevice {
                    path: std::ffi::CString::new(d.path).unwrap().into_raw(),
                    uuid: d.uuid,
                },
                cdc: CdcDevice {
                    path: std::ptr::null(),
                    uuid: 0,
                },
            },
            odyssey_hub_common::device::Device::Cdc(d) => Device {
                kind: DeviceKind::Cdc,
                udp: UdpDevice {
                    id: 0,
                    addr: std::ptr::null(),
                    uuid: 0,
                },
                hid: HidDevice {
                    path: std::ptr::null(),
                    uuid: 0,
                },
                cdc: CdcDevice {
                    path: std::ffi::CString::new(d.path).unwrap().into_raw(),
                    uuid: d.uuid,
                },
            },
        }
    }
}

impl From<Device> for odyssey_hub_common::device::Device {
    fn from(device: Device) -> Self {
        match device.kind {
            DeviceKind::Udp => {
                odyssey_hub_common::device::Device::Udp(odyssey_hub_common::device::UdpDevice {
                    id: device.udp.id,
                    addr: unsafe { std::ffi::CStr::from_ptr(device.udp.addr) }
                        .to_str()
                        .unwrap()
                        .parse()
                        .unwrap(),
                    uuid: device.udp.uuid,
                })
            }
            DeviceKind::Hid => {
                odyssey_hub_common::device::Device::Hid(odyssey_hub_common::device::HidDevice {
                    path: unsafe { std::ffi::CStr::from_ptr(device.hid.path) }
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    uuid: device.hid.uuid,
                })
            }
            DeviceKind::Cdc => {
                odyssey_hub_common::device::Device::Cdc(odyssey_hub_common::device::CdcDevice {
                    path: unsafe { std::ffi::CStr::from_ptr(device.cdc.path) }
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    uuid: device.cdc.uuid,
                })
            }
        }
    }
}

impl From<odyssey_hub_common::events::Event> for Event {
    fn from(event: odyssey_hub_common::events::Event) -> Self {
        match event {
            odyssey_hub_common::events::Event::DeviceEvent(device_event) => Event {
                kind: EventKind::DeviceEvent(device_event.into()),
            },
        }
    }
}

impl From<odyssey_hub_common::events::DeviceEvent> for DeviceEvent {
    fn from(event: odyssey_hub_common::events::DeviceEvent) -> Self {
        DeviceEvent {
            device: event.0.into(),
            kind: event.1.into(),
        }
    }
}

impl From<odyssey_hub_common::events::DeviceEventKind> for DeviceEventKind {
    fn from(kind: odyssey_hub_common::events::DeviceEventKind) -> Self {
        match kind {
            odyssey_hub_common::events::DeviceEventKind::AccelerometerEvent(e) => {
                DeviceEventKind::AccelerometerEvent(e.into())
            }
            odyssey_hub_common::events::DeviceEventKind::TrackingEvent(e) => {
                DeviceEventKind::TrackingEvent(e.into())
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
            odyssey_hub_common::events::DeviceEventKind::ZeroResult(r) => {
                DeviceEventKind::ZeroResult(r)
            }
            odyssey_hub_common::events::DeviceEventKind::SaveZeroResult(r) => {
                DeviceEventKind::SaveZeroResult(r)
            }
            odyssey_hub_common::events::DeviceEventKind::ShotDelayChangedEvent(e) => {
                DeviceEventKind::ShotDelayChanged(e)
            }
            odyssey_hub_common::events::DeviceEventKind::PacketEvent(p) => {
                DeviceEventKind::PacketEvent(p.into())
            }
        }
    }
}

impl From<odyssey_hub_common::events::TrackingEvent> for TrackingEvent {
    fn from(event: odyssey_hub_common::events::TrackingEvent) -> Self {
        let (pose, has_pose) = match event.pose {
            Some(p) => (p.into(), true),
            None => (Pose::default(), false),
        };
        TrackingEvent {
            timestamp: event.timestamp,
            aimpoint: event.aimpoint.into(),
            pose,
            has_pose,
            distance: event.distance,
            screen_id: event.screen_id,
        }
    }
}

impl From<TrackingEvent> for odyssey_hub_common::events::TrackingEvent {
    fn from(event: TrackingEvent) -> Self {
        odyssey_hub_common::events::TrackingEvent {
            timestamp: event.timestamp,
            aimpoint: event.aimpoint.into(),
            pose: if event.has_pose { Some(event.pose.into()) } else { None },
            distance: event.distance,
            screen_id: event.screen_id,
        }
    }
}

impl From<ats_usb::packets::vm::Packet> for PacketEvent {
    fn from(packet: ats_usb::packets::vm::Packet) -> Self {
        PacketEvent {
            ty: packet.ty().into(),
            data: packet.data.into(),
        }
    }
}

#[no_mangle]
pub extern "C" fn device_uuid(d: Device) -> u64 {
    d.uuid()
}
