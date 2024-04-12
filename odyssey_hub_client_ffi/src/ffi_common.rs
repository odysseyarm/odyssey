use odyssey_hub_common::events::Pose;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct FfiDevice {
    tag: FfiDeviceTag,
    u: FfiU,
}

impl From<odyssey_hub_common::device::Device> for FfiDevice {
    fn from(device: odyssey_hub_common::device::Device) -> Self {
        match device {
            odyssey_hub_common::device::Device::Udp(udp_device) => FfiDevice {
                tag: FfiDeviceTag::Udp,
                u: FfiU {
                    udp: FfiUdpDevice {
                        id: udp_device.id,
                        addr: FfiSocketAddr {
                            ip: std::ffi::CString::new(udp_device.addr.ip().to_string()).unwrap().into_raw(),
                            port: udp_device.addr.port(),
                        },
                    },
                },
            },
            odyssey_hub_common::device::Device::Hid(hid_device) => FfiDevice {
                tag: FfiDeviceTag::Hid,
                u: FfiU {
                    hid: FfiHidDevice {
                        path: std::ffi::CString::new(hid_device.path).unwrap().into_raw(),
                    },
                },
            },
            odyssey_hub_common::device::Device::Cdc(cdc_device) => FfiDevice {
                tag: FfiDeviceTag::Cdc,
                u: FfiU {
                    cdc: FfiCdcDevice {
                        path: std::ffi::CString::new(cdc_device.path).unwrap().into_raw(),
                    },
                },
            },
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union FfiU {
    udp: FfiUdpDevice,
    hid: FfiHidDevice,
    cdc: FfiCdcDevice,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct FfiUdpDevice {
    pub id: u8,
    pub addr: FfiSocketAddr,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct FfiHidDevice {
    pub path: *const std::ffi::c_char,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct FfiCdcDevice {
    pub path: *const std::ffi::c_char,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub enum FfiDeviceTag {
    Udp,
    Hid,
    Cdc,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct FfiSocketAddr {
    pub ip: *const std::ffi::c_char,
    pub port: u16,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct FfiEvent {
    tag: FfiEventTag,
    u: FfiEventU,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub enum FfiEventTag {
    None,
    DeviceEvent,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union FfiEventU {
    none: (),
    device_event: FfiDeviceEvent,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct FfiDeviceEvent {
    device: FfiDevice,
    kind: FfiDeviceEventKind,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct FfiDeviceEventKind {
    tag: FfiDeviceEventKindTag,
    u: FfiDeviceEventKindU,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub enum FfiDeviceEventKindTag {
    TrackingEvent,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union FfiDeviceEventKindU {
    tracking_event: FfiTrackingEvent,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct FfiTrackingEvent {
    aimpoint: nalgebra::Vector2<f64>,
    pose: FfiPose,
    pose_resolved: bool,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct FfiPose {
    rotation: crate::funny::Matrix3f64,
    translation: crate::funny::Matrix3x1f64,
}

impl From<Pose> for FfiPose {
    fn from(pose: Pose) -> Self {
        FfiPose {
            rotation: pose.rotation.into(),
            translation: pose.translation.into(),
        }
    }
}

impl From<odyssey_hub_common::events::Event> for FfiEvent {
    fn from(event: odyssey_hub_common::events::Event) -> Self {
        match event {
            odyssey_hub_common::events::Event::None => FfiEvent {
                tag: FfiEventTag::None,
                u: FfiEventU { none: () },
            },
            odyssey_hub_common::events::Event::DeviceEvent(device_event) => FfiEvent {
                tag: FfiEventTag::DeviceEvent,
                u: FfiEventU {
                    device_event: FfiDeviceEvent {
                        device: device_event.device.into(),
                        kind: FfiDeviceEventKind {
                            tag: match device_event.kind {
                                odyssey_hub_common::events::DeviceEventKind::TrackingEvent(_) => FfiDeviceEventKindTag::TrackingEvent,
                            },
                            u: match device_event.kind {
                                odyssey_hub_common::events::DeviceEventKind::TrackingEvent(tracking_event) => FfiDeviceEventKindU {
                                    tracking_event: FfiTrackingEvent {
                                        aimpoint: tracking_event.aimpoint.into(),
                                        pose: if let Some(p) = tracking_event.pose { p.into() } else { FfiPose::default() },
                                        pose_resolved: tracking_event.pose.is_some(),
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
