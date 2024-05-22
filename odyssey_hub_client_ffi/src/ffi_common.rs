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
                            ip: std::ffi::CString::new(udp_device.addr.ip().to_string()).unwrap().into_raw(),
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
    TrackingEvent,
    ImpactEvent,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union DeviceEventKindU {
    tracking_event: TrackingEvent,
    impact_event: ImpactEvent,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct TrackingEvent {
    timestamp: u32,
    aimpoint: crate::funny::Vector2f64,
    pose: Pose,
    pose_resolved: bool,
    screen_id: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct ImpactEvent {
    timestamp: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct Pose {
    rotation: crate::funny::Matrix3f64,
    translation: crate::funny::Matrix3x1f64,
}

impl From<odyssey_hub_common::events::Pose> for Pose {
    fn from(pose: odyssey_hub_common::events::Pose) -> Self {
        Pose {
            rotation: pose.rotation.into(),
            translation: pose.translation.into(),
        }
    }
}

impl From<odyssey_hub_common::events::Event> for Event {
    fn from(event: odyssey_hub_common::events::Event) -> Self {
        match event {
            odyssey_hub_common::events::Event::None => Event {
                tag: EventTag::None,
                u: EventU { none: 0, },
            },
            odyssey_hub_common::events::Event::DeviceEvent(device_event) => Event {
                tag: EventTag::DeviceEvent,
                u: EventU {
                    device_event: DeviceEvent {
                        device: device_event.device.into(),
                        kind: DeviceEventKind {
                            tag: match device_event.kind {
                                odyssey_hub_common::events::DeviceEventKind::TrackingEvent(_) => DeviceEventKindTag::TrackingEvent,
                                odyssey_hub_common::events::DeviceEventKind::ImpactEvent(_) => DeviceEventKindTag::ImpactEvent,
                            },
                            u: match device_event.kind {
                                odyssey_hub_common::events::DeviceEventKind::TrackingEvent(tracking_event) => DeviceEventKindU {
                                    tracking_event: TrackingEvent {
                                        timestamp: tracking_event.timestamp,
                                        aimpoint: tracking_event.aimpoint.into(),
                                        pose: if let Some(p) = tracking_event.pose { p.into() } else { Pose::default() },
                                        pose_resolved: tracking_event.pose.is_some(),
                                        screen_id: tracking_event.screen_id,
                                    },
                                },
                                odyssey_hub_common::events::DeviceEventKind::ImpactEvent(impact_event) => DeviceEventKindU {
                                    impact_event: ImpactEvent {
                                        timestamp: impact_event.timestamp,
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
