use std::net::SocketAddr;

mod proto {
    tonic::include_proto!("odyssey.service_interface");
}

pub use proto::*;

impl From<nalgebra::Matrix3x1<f64>> for proto::Matrix3x1 {
    fn from(value: nalgebra::Matrix3x1<f64>) -> Self {
        Self {
            m11: value.x,
            m21: value.y,
            m31: value.z,
        }
    }
}

impl Into<nalgebra::Matrix3x1<f64>> for proto::Matrix3x1 {
    fn into(self) -> nalgebra::Matrix3x1<f64> {
        nalgebra::Matrix3x1::new(self.m11, self.m21, self.m31)
    }
}

impl From<nalgebra::Matrix3<f64>> for proto::Matrix3x3 {
    fn from(value: nalgebra::Matrix3<f64>) -> Self {
        Self {
            m11: value.m11,
            m12: value.m12,
            m13: value.m13,
            m21: value.m21,
            m22: value.m22,
            m23: value.m23,
            m31: value.m31,
            m32: value.m32,
            m33: value.m33,
        }
    }
}

impl Into<nalgebra::Matrix3<f64>> for proto::Matrix3x3 {
    fn into(self) -> nalgebra::Matrix3<f64> {
        nalgebra::Matrix3::new(
            self.m11, self.m12, self.m13,
            self.m21, self.m22, self.m23,
            self.m31, self.m32, self.m33,
        )
    }
}

impl From<odyssey_hub_common::device::Device> for proto::Device {
    fn from(value: odyssey_hub_common::device::Device) -> Self {
        match value {
            odyssey_hub_common::device::Device::Udp(udp_device) => {
                proto::Device {
                    device_oneof: Some(proto::device::DeviceOneof::UdpDevice(proto::UdpDevice {
                        id: udp_device.id as i32,
                        ip: udp_device.addr.ip().to_string(),
                        port: udp_device.addr.port() as i32,
                    })),
                }
            },
            odyssey_hub_common::device::Device::Hid(d) => proto::Device {
                device_oneof: Some(proto::device::DeviceOneof::HidDevice(proto::HidDevice { path: d.path })),
            },
            odyssey_hub_common::device::Device::Cdc(d) => proto::Device {
                device_oneof: Some(proto::device::DeviceOneof::CdcDevice(proto::CdcDevice { path: d.path })),
            },
        }
    }
}
impl Into<odyssey_hub_common::device::Device> for proto::Device {
    fn into(self) -> odyssey_hub_common::device::Device {
        match self.device_oneof.unwrap() {
            proto::device::DeviceOneof::UdpDevice(proto::UdpDevice { id, ip, port }) => odyssey_hub_common::device::Device::Udp(odyssey_hub_common::device::UdpDevice {
                id: id as u8,
                addr: SocketAddr::new(ip.parse().unwrap(), port as u16),
            }),
            proto::device::DeviceOneof::HidDevice(proto::HidDevice { path }) => odyssey_hub_common::device::Device::Hid(odyssey_hub_common::device::HidDevice { path: path }),
            proto::device::DeviceOneof::CdcDevice(proto::CdcDevice { path }) => odyssey_hub_common::device::Device::Cdc(odyssey_hub_common::device::CdcDevice { path: path }),
        }
    }
}

impl From<odyssey_hub_common::events::Event> for proto::Event {
    fn from(value: odyssey_hub_common::events::Event) -> Self {
        match value {
            odyssey_hub_common::events::Event::None => proto::Event {
                event_oneof: None,
            },
            odyssey_hub_common::events::Event::DeviceEvent(odyssey_hub_common::events::DeviceEvent { device, kind }) => {
                proto::Event {
                    event_oneof: Some(
                        proto::event::EventOneof::Device(
                            proto::DeviceEvent {
                                device: Some(device.into()),
                                device_event_oneof: Some(match kind {
                                    odyssey_hub_common::events::DeviceEventKind::TrackingEvent(odyssey_hub_common::events::TrackingEvent { aimpoint, pose }) => {
                                        proto::device_event::DeviceEventOneof::Tracking(
                                            proto::device_event::TrackingEvent {
                                                aimpoint: Some(proto::Vector2 {
                                                    x: aimpoint.x,
                                                    y: aimpoint.y,
                                                }),
                                                pose: Some(proto::Pose {
                                                    rotation: Some(pose.unwrap().rotation.into()),
                                                    translation: Some(pose.unwrap().translation.into()),
                                                }),
                                            },
                                        )
                                    }
                                })
                            }
                        ),
                    )
                }
            },
        }
    }
}

impl Into<odyssey_hub_common::events::Event> for proto::Event {
    fn into(self) -> odyssey_hub_common::events::Event {
        match self.event_oneof.unwrap() {
            proto::event::EventOneof::Device(proto::DeviceEvent { device, device_event_oneof }) => {
                odyssey_hub_common::events::Event::DeviceEvent(
                    odyssey_hub_common::events::DeviceEvent {
                        device: device.unwrap().into(),
                        kind: match device_event_oneof.unwrap() {
                            proto::device_event::DeviceEventOneof::Tracking(proto::device_event::TrackingEvent { aimpoint, pose }) => {
                                odyssey_hub_common::events::DeviceEventKind::TrackingEvent(
                                    odyssey_hub_common::events::TrackingEvent {
                                        aimpoint: nalgebra::Vector2::new(aimpoint.clone().unwrap().x, aimpoint.clone().unwrap().y),
                                        pose: if let Some(pose) = pose {
                                            Some(odyssey_hub_common::events::Pose {
                                                rotation: pose.rotation.unwrap().into(),
                                                translation: pose.translation.unwrap().into(),
                                            })
                                        } else {
                                            None
                                        },
                                    },
                                )
                            }
                        }
                    }
                )
            }
        }
    }
}
