use std::{ffi::CStr, net::IpAddr};

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Device {
    tag: DeviceTag,
    u: DeviceU,
}

impl From<odyssey_hub_common::device::Device> for Device {
    fn from(device: odyssey_hub_common::device::Device) -> Self {
        match device {
            odyssey_hub_common::device::Device::Udp(udp_device) => Device {
                tag: DeviceTag::Udp,
                u: DeviceU {
                    udp: UdpDevice {
                        id: udp_device.id,
                        addr: SocketAddr {
                            ip: std::ffi::CString::new(udp_device.addr.ip().to_string())
                                .unwrap()
                                .into_raw(),
                            port: udp_device.addr.port(),
                        },
                        uuid: udp_device.uuid,
                    },
                },
            },
            odyssey_hub_common::device::Device::Hid(hid_device) => Device {
                tag: DeviceTag::Hid,
                u: DeviceU {
                    hid: HidDevice {
                        path: std::ffi::CString::new(hid_device.path).unwrap().into_raw(),
                        uuid: hid_device.uuid,
                    },
                },
            },
            odyssey_hub_common::device::Device::Cdc(cdc_device) => Device {
                tag: DeviceTag::Cdc,
                u: DeviceU {
                    cdc: CdcDevice {
                        path: std::ffi::CString::new(cdc_device.path).unwrap().into_raw(),
                        uuid: cdc_device.uuid,
                    },
                },
            },
        }
    }
}

impl From<Device> for odyssey_hub_common::device::Device {
    fn from(device: Device) -> Self {
        match device.tag {
            DeviceTag::Udp => unsafe {
                // Access the `udp` field from the union
                let udp = device.u.udp;

                // Reconstruct the CString from the raw pointer to manage memory correctly
                let ip_cstring = CStr::from_ptr(udp.addr.ip);
                let ip_str = ip_cstring.to_str().unwrap();
                let ip_addr: IpAddr = ip_str.parse().unwrap();

                // Construct the standard SocketAddr
                let socket_addr = std::net::SocketAddr::new(ip_addr, udp.addr.port);

                // Create the UdpDevice
                odyssey_hub_common::device::Device::Udp(odyssey_hub_common::device::UdpDevice {
                    id: udp.id,
                    addr: socket_addr,
                    uuid: udp.uuid,
                })
            },
            DeviceTag::Hid => unsafe {
                // Access the `hid` field from the union
                let hid = device.u.hid;

                // Reconstruct the CString from the raw pointer
                let path_cstring = CStr::from_ptr(hid.path);
                let path_str = path_cstring.to_str().unwrap().to_string();

                // Create the HidDevice
                odyssey_hub_common::device::Device::Hid(odyssey_hub_common::device::HidDevice {
                    path: path_str,
                    uuid: hid.uuid,
                })
            },
            DeviceTag::Cdc => unsafe {
                // Access the `cdc` field from the union
                let cdc = device.u.cdc;

                // Reconstruct the CString from the raw pointer
                let path_cstring = CStr::from_ptr(cdc.path);
                let path_str = path_cstring.to_str().unwrap().to_string();

                // Create the CdcDevice
                odyssey_hub_common::device::Device::Cdc(odyssey_hub_common::device::CdcDevice {
                    path: path_str,
                    uuid: cdc.uuid,
                })
            },
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union DeviceU {
    udp: UdpDevice,
    hid: HidDevice,
    cdc: CdcDevice,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct UdpDevice {
    pub id: u8,
    pub addr: SocketAddr,
    pub uuid: [u8; 6],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct HidDevice {
    pub path: *const std::ffi::c_char,
    pub uuid: [u8; 6],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct CdcDevice {
    pub path: *const std::ffi::c_char,
    pub uuid: [u8; 6],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub enum DeviceTag {
    Udp,
    Hid,
    Cdc,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct SocketAddr {
    pub ip: *const std::ffi::c_char,
    pub port: u16,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Event {
    tag: EventTag,
    u: EventU,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub enum EventTag {
    None,
    DeviceEvent,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union EventU {
    // C# doesn't support void type
    none: u8,
    device_event: DeviceEvent,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct DeviceEvent {
    device: Device,
    kind: DeviceEventKind,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct DeviceEventKind {
    tag: DeviceEventKindTag,
    u: DeviceEventKindU,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub enum DeviceEventKindTag {
    AccelerometerEvent,
    TrackingEvent,
    ImpactEvent,
    ConnectEvent,
    DisconnectEvent,
    PacketEvent,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union DeviceEventKindU {
    accelerometer_event: AccelerometerEvent,
    tracking_event: TrackingEvent,
    impact_event: ImpactEvent,
    connect_event: ConnectEvent,
    disconnect_event: DisconnectEvent,
    packet_event: PacketEvent,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct AccelerometerEvent {
    timestamp: u32,
    accel: crate::funny::Vector3f32,
    gyro: crate::funny::Vector3f32,
    euler_angles: crate::funny::Vector3f32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct TrackingEvent {
    timestamp: u32,
    aimpoint: crate::funny::Vector2f32,
    pose: Pose,
    pose_resolved: bool,
    distance: f32,
    screen_id: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct ImpactEvent {
    timestamp: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct PacketEvent {
    ty: u8,
    data: PacketData,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct ConnectEvent {
    _unused: u8,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct DisconnectEvent {
    _unused: u8,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct Pose {
    rotation: crate::funny::Matrix3f32,
    translation: crate::funny::Matrix3x1f32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct PacketData {
    tag: PacketDataTag,
    u: PacketDataU,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub enum PacketDataTag {
    Unsupported,
    VendorEvent,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union PacketDataU {
    unsupported: UnsupportedPacketData,
    vendor_event: VendorEventPacketData,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct UnsupportedPacketData {
    _unused: u8,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct VendorEventPacketData {
    pub len: u8,
    pub data: [u8; 98],
}

impl From<odyssey_hub_common::events::Pose> for Pose {
    fn from(pose: odyssey_hub_common::events::Pose) -> Self {
        Pose {
            rotation: pose.rotation.into(),
            translation: pose.translation.into(),
        }
    }
}

impl From<ats_usb::packet::PacketData> for PacketData {
    fn from(packet_data: ats_usb::packet::PacketData) -> Self {
        match packet_data {
            ats_usb::packet::PacketData::Vendor(_, (len, data)) => PacketData {
                tag: PacketDataTag::VendorEvent,
                u: PacketDataU {
                    vendor_event: VendorEventPacketData { len, data },
                },
            },
            _ => PacketData {
                tag: PacketDataTag::Unsupported,
                u: PacketDataU {
                    unsupported: UnsupportedPacketData { _unused: 0 },
                },
            },
        }
    }
}

impl From<odyssey_hub_common::events::Event> for Event {
    fn from(event: odyssey_hub_common::events::Event) -> Self {
        match event {
            odyssey_hub_common::events::Event::None => Event {
                tag: EventTag::None,
                u: EventU { none: 0 },
            },
            odyssey_hub_common::events::Event::DeviceEvent(device_event) => Event {
                tag: EventTag::DeviceEvent,
                u: EventU {
                    device_event: DeviceEvent {
                        device: device_event.device.into(),
                        kind: DeviceEventKind {
                            tag: match device_event.kind {
                                odyssey_hub_common::events::DeviceEventKind::AccelerometerEvent(
                                    _,
                                ) => DeviceEventKindTag::AccelerometerEvent,
                                odyssey_hub_common::events::DeviceEventKind::TrackingEvent(_) => {
                                    DeviceEventKindTag::TrackingEvent
                                }
                                odyssey_hub_common::events::DeviceEventKind::ImpactEvent(_) => {
                                    DeviceEventKindTag::ImpactEvent
                                }
                                odyssey_hub_common::events::DeviceEventKind::ConnectEvent => {
                                    DeviceEventKindTag::ConnectEvent
                                }
                                odyssey_hub_common::events::DeviceEventKind::DisconnectEvent => {
                                    DeviceEventKindTag::DisconnectEvent
                                }
                                odyssey_hub_common::events::DeviceEventKind::PacketEvent(_) => {
                                    DeviceEventKindTag::PacketEvent
                                }
                            },
                            u: match device_event.kind {
                                odyssey_hub_common::events::DeviceEventKind::AccelerometerEvent(
                                    accelerometer_event,
                                ) => DeviceEventKindU {
                                    accelerometer_event: AccelerometerEvent {
                                        timestamp: accelerometer_event.timestamp,
                                        accel: accelerometer_event.accel.into(),
                                        gyro: accelerometer_event.gyro.into(),
                                        euler_angles: accelerometer_event.euler_angles.into(),
                                    },
                                },
                                odyssey_hub_common::events::DeviceEventKind::TrackingEvent(
                                    tracking_event,
                                ) => DeviceEventKindU {
                                    tracking_event: TrackingEvent {
                                        timestamp: tracking_event.timestamp,
                                        aimpoint: tracking_event.aimpoint.into(),
                                        pose: if let Some(p) = tracking_event.pose {
                                            p.into()
                                        } else {
                                            Pose::default()
                                        },
                                        pose_resolved: tracking_event.pose.is_some(),
                                        distance: tracking_event.distance,
                                        screen_id: tracking_event.screen_id,
                                    },
                                },
                                odyssey_hub_common::events::DeviceEventKind::ImpactEvent(
                                    impact_event,
                                ) => DeviceEventKindU {
                                    impact_event: ImpactEvent {
                                        timestamp: impact_event.timestamp,
                                    },
                                },
                                odyssey_hub_common::events::DeviceEventKind::ConnectEvent => {
                                    DeviceEventKindU {
                                        connect_event: ConnectEvent { _unused: 0 },
                                    }
                                }
                                odyssey_hub_common::events::DeviceEventKind::DisconnectEvent => {
                                    DeviceEventKindU {
                                        disconnect_event: DisconnectEvent { _unused: 0 },
                                    }
                                }
                                odyssey_hub_common::events::DeviceEventKind::PacketEvent(
                                    packet_event,
                                ) => DeviceEventKindU {
                                    packet_event: PacketEvent {
                                        ty: packet_event.ty().into(),
                                        data: packet_event.data.into(),
                                    },
                                },
                            },
                        },
                    },
                },
            },
        }
    }
}
