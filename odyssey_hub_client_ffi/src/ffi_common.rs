use std::ffi::c_char;

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
        common::device::Device::from(self).uuid()
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

type DeviceId = u64;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AccessoryInfo {
    pub name: *const c_char, // C string
    pub ty: odyssey_hub_common::AccessoryType,
    pub assignment: DeviceId,
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
    ShotDelayChanged(u16),
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

impl_from_simple!(common::events::AccelerometerEvent => AccelerometerEvent, timestamp, accel, gyro, euler_angles);
impl_from_simple!(common::events::ImpactEvent => ImpactEvent, timestamp);
impl_from_simple!(common::events::Pose => Pose, rotation, translation);
impl_from_simple!(Pose => common::events::Pose, rotation, translation);

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

impl From<common::device::Device> for Device {
    fn from(device: common::device::Device) -> Self {
        match device {
            common::device::Device::Udp(d) => Device {
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
            common::device::Device::Hid(d) => Device {
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
            common::device::Device::Cdc(d) => Device {
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

impl From<Device> for common::device::Device {
    fn from(device: Device) -> Self {
        match device.kind {
            DeviceKind::Udp => common::device::Device::Udp(common::device::UdpDevice {
                id: device.udp.id,
                addr: unsafe { std::ffi::CStr::from_ptr(device.udp.addr) }
                    .to_str()
                    .unwrap()
                    .parse()
                    .unwrap(),
                uuid: device.udp.uuid,
            }),
            DeviceKind::Hid => common::device::Device::Hid(common::device::HidDevice {
                path: unsafe { std::ffi::CStr::from_ptr(device.hid.path) }
                    .to_str()
                    .unwrap()
                    .to_owned(),
                uuid: device.hid.uuid,
            }),
            DeviceKind::Cdc => common::device::Device::Cdc(common::device::CdcDevice {
                path: unsafe { std::ffi::CStr::from_ptr(device.cdc.path) }
                    .to_str()
                    .unwrap()
                    .to_owned(),
                uuid: device.cdc.uuid,
            }),
        }
    }
}

impl From<common::events::Event> for Event {
    fn from(event: common::events::Event) -> Self {
        match event {
            common::events::Event::DeviceEvent(device_event) => Event {
                kind: EventKind::DeviceEvent(device_event.into()),
            },
        }
    }
}

impl From<common::events::DeviceEvent> for DeviceEvent {
    fn from(event: common::events::DeviceEvent) -> Self {
        DeviceEvent {
            device: event.0.into(),
            kind: event.1.into(),
        }
    }
}

impl From<common::events::DeviceEventKind> for DeviceEventKind {
    fn from(kind: common::events::DeviceEventKind) -> Self {
        match kind {
            common::events::DeviceEventKind::AccelerometerEvent(e) => {
                DeviceEventKind::AccelerometerEvent(e.into())
            }
            common::events::DeviceEventKind::TrackingEvent(e) => {
                DeviceEventKind::TrackingEvent(e.into())
            }
            common::events::DeviceEventKind::ImpactEvent(e) => {
                DeviceEventKind::ImpactEvent(e.into())
            }
            common::events::DeviceEventKind::ConnectEvent => DeviceEventKind::ConnectEvent,
            common::events::DeviceEventKind::DisconnectEvent => DeviceEventKind::DisconnectEvent,
            common::events::DeviceEventKind::ZeroResult(r) => DeviceEventKind::ZeroResult(r),
            common::events::DeviceEventKind::SaveZeroResult(r) => {
                DeviceEventKind::SaveZeroResult(r)
            }
            common::events::DeviceEventKind::ShotDelayChangedEvent(e) => {
                DeviceEventKind::ShotDelayChanged(e)
            }
            common::events::DeviceEventKind::PacketEvent(p) => {
                DeviceEventKind::PacketEvent(p.into())
            }
        }
    }
}

impl From<common::events::TrackingEvent> for TrackingEvent {
    fn from(event: common::events::TrackingEvent) -> Self {
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

impl From<TrackingEvent> for common::events::TrackingEvent {
    fn from(event: TrackingEvent) -> Self {
        common::events::TrackingEvent {
            timestamp: event.timestamp,
            aimpoint: event.aimpoint.into(),
            pose: if event.has_pose {
                Some(event.pose.into())
            } else {
                None
            },
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

impl From<common::AccessoryInfo> for AccessoryInfo {
    fn from(info: common::AccessoryInfo) -> Self {
        AccessoryInfo {
            name: std::ffi::CString::new(info.name).unwrap().into_raw(),
            ty: info.ty,
            assignment: if let Some(assignment) = info.assignment {
                assignment.get()
            } else {
                0
            },
        }
    }
}

#[no_mangle]
pub extern "C" fn device_uuid(d: Device) -> u64 {
    d.uuid()
}
