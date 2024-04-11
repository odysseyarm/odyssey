#[repr(C)]
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
                    udp: FFiUdpDevice {
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
pub union FfiU {
    udp: FFiUdpDevice,
    hid: FfiHidDevice,
    cdc: FfiCdcDevice,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct FFiUdpDevice {
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
