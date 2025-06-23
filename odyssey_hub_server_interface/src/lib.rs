use std::net::SocketAddr;

mod proto {
    tonic::include_proto!("odyssey.service_interface");
}

pub use proto::*;

impl From<nalgebra::Matrix3x1<f32>> for proto::Matrix3x1 {
    fn from(value: nalgebra::Matrix3x1<f32>) -> Self {
        Self {
            m11: value.x,
            m21: value.y,
            m31: value.z,
        }
    }
}

impl Into<nalgebra::Matrix3x1<f32>> for proto::Matrix3x1 {
    fn into(self) -> nalgebra::Matrix3x1<f32> {
        nalgebra::Matrix3x1::new(self.m11, self.m21, self.m31)
    }
}

impl From<nalgebra::Matrix3<f32>> for proto::Matrix3x3 {
    fn from(value: nalgebra::Matrix3<f32>) -> Self {
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

impl Into<nalgebra::Matrix3<f32>> for proto::Matrix3x3 {
    fn into(self) -> nalgebra::Matrix3<f32> {
        nalgebra::Matrix3::new(
            self.m11, self.m12, self.m13, self.m21, self.m22, self.m23, self.m31, self.m32,
            self.m33,
        )
    }
}

impl From<nalgebra::Vector2<f32>> for proto::Vector2 {
    fn from(value: nalgebra::Vector2<f32>) -> Self {
        Self {
            x: value.x,
            y: value.y,
        }
    }
}

impl Into<nalgebra::Vector2<f32>> for proto::Vector2 {
    fn into(self) -> nalgebra::Vector2<f32> {
        nalgebra::Vector2::new(self.x, self.y)
    }
}

impl From<nalgebra::Vector3<f32>> for proto::Vector3 {
    fn from(value: nalgebra::Vector3<f32>) -> Self {
        Self {
            x: value.x,
            y: value.y,
            z: value.z,
        }
    }
}

impl Into<nalgebra::Vector3<f32>> for proto::Vector3 {
    fn into(self) -> nalgebra::Vector3<f32> {
        nalgebra::Vector3::new(self.x, self.y, self.z)
    }
}

impl From<odyssey_hub_common::device::Device> for proto::Device {
    fn from(value: odyssey_hub_common::device::Device) -> Self {
        match value {
            odyssey_hub_common::device::Device::Udp(d) => proto::Device {
                device_oneof: Some(proto::device::DeviceOneof::UdpDevice(proto::UdpDevice {
                    id: d.id as i32,
                    ip: d.addr.ip().to_string(),
                    port: d.addr.port() as i32,
                    uuid: d.uuid,
                })),
            },
            odyssey_hub_common::device::Device::Hid(d) => proto::Device {
                device_oneof: Some(proto::device::DeviceOneof::HidDevice(proto::HidDevice {
                    path: d.path,
                    uuid: d.uuid,
                })),
            },
            odyssey_hub_common::device::Device::Cdc(d) => proto::Device {
                device_oneof: Some(proto::device::DeviceOneof::CdcDevice(proto::CdcDevice {
                    path: d.path,
                    uuid: d.uuid,
                })),
            },
        }
    }
}

impl From<proto::Device> for odyssey_hub_common::device::Device {
    fn from(value: proto::Device) -> Self {
        match value.device_oneof.unwrap() {
            proto::device::DeviceOneof::UdpDevice(proto::UdpDevice { id, ip, port, uuid }) => {
                odyssey_hub_common::device::Device::Udp(odyssey_hub_common::device::UdpDevice {
                    uuid,
                    id: id as u8,
                    addr: SocketAddr::new(ip.parse().unwrap(), port as u16),
                })
            }
            proto::device::DeviceOneof::HidDevice(proto::HidDevice { path, uuid }) => {
                odyssey_hub_common::device::Device::Hid(odyssey_hub_common::device::HidDevice {
                    uuid,
                    path,
                })
            }
            proto::device::DeviceOneof::CdcDevice(proto::CdcDevice { path, uuid }) => {
                odyssey_hub_common::device::Device::Cdc(odyssey_hub_common::device::CdcDevice {
                    uuid,
                    path,
                })
            }
        }
    }
}

impl From<odyssey_hub_common::events::Event> for proto::Event {
    fn from(value: odyssey_hub_common::events::Event) -> Self {
        match value {
            odyssey_hub_common::events::Event::DeviceEvent(
                odyssey_hub_common::events::DeviceEvent(device, kind),
            ) => proto::Event {
                event_oneof: Some(proto::event::EventOneof::Device(proto::DeviceEvent {
                    device: Some(device.into()),
                    device_event_oneof: Some(match kind {
                        odyssey_hub_common::events::DeviceEventKind::AccelerometerEvent(
                            odyssey_hub_common::events::AccelerometerEvent {
                                timestamp,
                                accel,
                                gyro,
                                euler_angles,
                            },
                        ) => proto::device_event::DeviceEventOneof::Accelerometer(
                            proto::device_event::AccelerometerEvent {
                                timestamp,
                                acceleration: Some(proto::Vector3 {
                                    x: accel.x,
                                    y: accel.y,
                                    z: accel.z,
                                }),
                                angular_velocity: Some(proto::Vector3 {
                                    x: gyro.x,
                                    y: gyro.y,
                                    z: gyro.z,
                                }),
                                euler_angles: Some(proto::Vector3 {
                                    x: euler_angles.x,
                                    y: euler_angles.y,
                                    z: euler_angles.z,
                                }),
                            },
                        ),
                        odyssey_hub_common::events::DeviceEventKind::TrackingEvent(
                            odyssey_hub_common::events::TrackingEvent {
                                timestamp,
                                aimpoint,
                                pose,
                                distance,
                                screen_id,
                            },
                        ) => proto::device_event::DeviceEventOneof::Tracking(
                            proto::device_event::TrackingEvent {
                                timestamp,
                                aimpoint: Some(proto::Vector2 {
                                    x: aimpoint.x,
                                    y: aimpoint.y,
                                }),
                                pose: Some(proto::Pose {
                                    rotation: Some(pose.unwrap().rotation.into()),
                                    translation: Some(pose.unwrap().translation.into()),
                                }),
                                distance,
                                screen_id,
                            },
                        ),
                        odyssey_hub_common::events::DeviceEventKind::ImpactEvent(
                            odyssey_hub_common::events::ImpactEvent { timestamp },
                        ) => proto::device_event::DeviceEventOneof::Impact(
                            proto::device_event::ImpactEvent { timestamp },
                        ),
                        odyssey_hub_common::events::DeviceEventKind::ConnectEvent => {
                            proto::device_event::DeviceEventOneof::Connect(
                                proto::device_event::ConnectEvent {},
                            )
                        }
                        odyssey_hub_common::events::DeviceEventKind::DisconnectEvent => {
                            proto::device_event::DeviceEventOneof::Disconnect(
                                proto::device_event::DisconnectEvent {},
                            )
                        }
                        odyssey_hub_common::events::DeviceEventKind::ZeroResult(success) => {
                            proto::device_event::DeviceEventOneof::ZeroResult(
                                proto::device_event::ZeroResultEvent { success },
                            )
                        }
                        odyssey_hub_common::events::DeviceEventKind::SaveZeroResult(success) => {
                            proto::device_event::DeviceEventOneof::SaveZeroResult(
                                proto::device_event::SaveZeroResultEvent { success },
                            )
                        }
                        odyssey_hub_common::events::DeviceEventKind::ShotDelayChangedEvent(delay_ms) => {
                            proto::device_event::DeviceEventOneof::ShotDelayChanged(
                                proto::device_event::ShotDelayChangedEvent { delay_ms: delay_ms.into() },
                            )
                        }
                        odyssey_hub_common::events::DeviceEventKind::PacketEvent(packet) => {
                            proto::device_event::DeviceEventOneof::Packet(
                                proto::device_event::PacketEvent {
                                    bytes: {
                                        let mut buf = Vec::new();
                                        packet.serialize(&mut buf);
                                        buf
                                    },
                                },
                            )
                        }
                    }),
                })),
            },
        }
    }
}

impl From<proto::Event> for odyssey_hub_common::events::Event {
    fn from(event: proto::Event) -> Self {
        match event.event_oneof.unwrap() {
            proto::event::EventOneof::Device(proto::DeviceEvent {
                device,
                device_event_oneof,
            }) => odyssey_hub_common::events::Event::DeviceEvent(
                odyssey_hub_common::events::DeviceEvent(
                    device.unwrap().into(),
                    match device_event_oneof.unwrap() {
                        proto::device_event::DeviceEventOneof::Accelerometer(
                            proto::device_event::AccelerometerEvent {
                                timestamp,
                                acceleration,
                                angular_velocity,
                                euler_angles,
                            },
                        ) => odyssey_hub_common::events::DeviceEventKind::AccelerometerEvent(
                            odyssey_hub_common::events::AccelerometerEvent {
                                timestamp,
                                accel: nalgebra::Vector3::new(
                                    acceleration.clone().unwrap().x,
                                    acceleration.clone().unwrap().y,
                                    acceleration.clone().unwrap().z,
                                ),
                                gyro: nalgebra::Vector3::new(
                                    angular_velocity.clone().unwrap().x,
                                    angular_velocity.clone().unwrap().y,
                                    angular_velocity.clone().unwrap().z,
                                ),
                                euler_angles: nalgebra::Vector3::new(
                                    euler_angles.clone().unwrap().x,
                                    euler_angles.clone().unwrap().y,
                                    euler_angles.clone().unwrap().z,
                                ),
                            },
                        ),
                        proto::device_event::DeviceEventOneof::Tracking(
                            proto::device_event::TrackingEvent {
                                timestamp,
                                aimpoint,
                                pose,
                                distance,
                                screen_id,
                            },
                        ) => odyssey_hub_common::events::DeviceEventKind::TrackingEvent(
                            odyssey_hub_common::events::TrackingEvent {
                                timestamp,
                                aimpoint: nalgebra::Vector2::new(
                                    aimpoint.clone().unwrap().x,
                                    aimpoint.clone().unwrap().y,
                                ),
                                pose: if let Some(pose) = pose {
                                    Some(odyssey_hub_common::events::Pose {
                                        rotation: pose.rotation.unwrap().into(),
                                        translation: pose.translation.unwrap().into(),
                                    })
                                } else {
                                    None
                                },
                                distance,
                                screen_id,
                            },
                        ),
                        proto::device_event::DeviceEventOneof::Impact(
                            proto::device_event::ImpactEvent { timestamp },
                        ) => odyssey_hub_common::events::DeviceEventKind::ImpactEvent(
                            odyssey_hub_common::events::ImpactEvent { timestamp },
                        ),
                        proto::device_event::DeviceEventOneof::Connect(_) => {
                            odyssey_hub_common::events::DeviceEventKind::ConnectEvent
                        }
                        proto::device_event::DeviceEventOneof::Disconnect(_) => {
                            odyssey_hub_common::events::DeviceEventKind::DisconnectEvent
                        }
                        proto::device_event::DeviceEventOneof::ZeroResult(proto::device_event::ZeroResultEvent { success }) => {
                            odyssey_hub_common::events::DeviceEventKind::ZeroResult(success)
                        }
                        proto::device_event::DeviceEventOneof::SaveZeroResult(proto::device_event::SaveZeroResultEvent { success }) => {
                            odyssey_hub_common::events::DeviceEventKind::SaveZeroResult(success)
                        }
                        proto::device_event::DeviceEventOneof::ShotDelayChanged(proto::device_event::ShotDelayChangedEvent { delay_ms }) => {
                            odyssey_hub_common::events::DeviceEventKind::ShotDelayChangedEvent(delay_ms as u8)
                        }
                        proto::device_event::DeviceEventOneof::Packet(
                            proto::device_event::PacketEvent { bytes },
                        ) => {
                            let mut bytes_slice: &[u8] = &bytes;
                            odyssey_hub_common::events::DeviceEventKind::PacketEvent(
                                ats_usb::packets::vm::Packet::parse(&mut bytes_slice).unwrap(),
                            )
                        }
                    },
                ),
            ),
        }
    }
}

impl From<proto::ScreenInfoResponse> for odyssey_hub_common::ScreenInfo {
    fn from(value: proto::ScreenInfoResponse) -> Self {
        let bounds = value.bounds.unwrap();
        odyssey_hub_common::ScreenInfo {
            id: value.id as u8,
            bounds: [
                bounds.tl.unwrap().into(),
                bounds.tr.unwrap().into(),
                bounds.bl.unwrap().into(),
                bounds.br.unwrap().into(),
            ],
        }
    }
}
