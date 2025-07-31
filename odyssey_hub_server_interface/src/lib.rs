use std::net::SocketAddr;

use std::num::NonZero;

mod proto {
    tonic::include_proto!("odyssey.service_interface");
}

pub use proto::*;

use odyssey_hub_common as common;

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

impl From<common::device::Device> for proto::Device {
    fn from(value: common::device::Device) -> Self {
        match value {
            common::device::Device::Udp(d) => proto::Device {
                device_oneof: Some(proto::device::DeviceOneof::UdpDevice(proto::UdpDevice {
                    id: d.id as i32,
                    ip: d.addr.ip().to_string(),
                    port: d.addr.port() as i32,
                    uuid: d.uuid,
                })),
            },
            common::device::Device::Hid(d) => proto::Device {
                device_oneof: Some(proto::device::DeviceOneof::HidDevice(proto::HidDevice {
                    path: d.path,
                    uuid: d.uuid,
                })),
            },
            common::device::Device::Cdc(d) => proto::Device {
                device_oneof: Some(proto::device::DeviceOneof::CdcDevice(proto::CdcDevice {
                    path: d.path,
                    uuid: d.uuid,
                })),
            },
        }
    }
}

impl From<proto::Device> for common::device::Device {
    fn from(value: proto::Device) -> Self {
        match value.device_oneof.unwrap() {
            proto::device::DeviceOneof::UdpDevice(proto::UdpDevice { id, ip, port, uuid }) => {
                common::device::Device::Udp(common::device::UdpDevice {
                    uuid,
                    id: id as u8,
                    addr: SocketAddr::new(ip.parse().unwrap(), port as u16),
                })
            }
            proto::device::DeviceOneof::HidDevice(proto::HidDevice { path, uuid }) => {
                common::device::Device::Hid(common::device::HidDevice { uuid, path })
            }
            proto::device::DeviceOneof::CdcDevice(proto::CdcDevice { path, uuid }) => {
                common::device::Device::Cdc(common::device::CdcDevice { uuid, path })
            }
        }
    }
}

impl From<common::events::Event> for proto::Event {
    fn from(value: common::events::Event) -> Self {
        match value {
            common::events::Event::DeviceEvent(common::events::DeviceEvent(device, kind)) => {
                proto::Event {
                    event_oneof: Some(proto::event::EventOneof::Device(proto::DeviceEvent {
                        device: Some(device.into()),
                        device_event_oneof: Some(match kind {
                            common::events::DeviceEventKind::AccelerometerEvent(
                                common::events::AccelerometerEvent {
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
                            common::events::DeviceEventKind::TrackingEvent(
                                common::events::TrackingEvent {
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
                            common::events::DeviceEventKind::ImpactEvent(
                                common::events::ImpactEvent { timestamp },
                            ) => proto::device_event::DeviceEventOneof::Impact(
                                proto::device_event::ImpactEvent { timestamp },
                            ),
                            common::events::DeviceEventKind::ConnectEvent => {
                                proto::device_event::DeviceEventOneof::Connect(
                                    proto::device_event::ConnectEvent {},
                                )
                            }
                            common::events::DeviceEventKind::DisconnectEvent => {
                                proto::device_event::DeviceEventOneof::Disconnect(
                                    proto::device_event::DisconnectEvent {},
                                )
                            }
                            common::events::DeviceEventKind::ZeroResult(success) => {
                                proto::device_event::DeviceEventOneof::ZeroResult(
                                    proto::device_event::ZeroResultEvent { success },
                                )
                            }
                            common::events::DeviceEventKind::SaveZeroResult(success) => {
                                proto::device_event::DeviceEventOneof::SaveZeroResult(
                                    proto::device_event::SaveZeroResultEvent { success },
                                )
                            }
                            common::events::DeviceEventKind::ShotDelayChangedEvent(delay_ms) => {
                                proto::device_event::DeviceEventOneof::ShotDelayChanged(
                                    proto::device_event::ShotDelayChangedEvent {
                                        delay_ms: delay_ms.into(),
                                    },
                                )
                            }
                            common::events::DeviceEventKind::PacketEvent(packet) => {
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
                }
            }
        }
    }
}

impl From<proto::Event> for common::events::Event {
    fn from(event: proto::Event) -> Self {
        match event.event_oneof.unwrap() {
            proto::event::EventOneof::Device(proto::DeviceEvent {
                device,
                device_event_oneof,
            }) => common::events::Event::DeviceEvent(common::events::DeviceEvent(
                device.unwrap().into(),
                match device_event_oneof.unwrap() {
                    proto::device_event::DeviceEventOneof::Accelerometer(
                        proto::device_event::AccelerometerEvent {
                            timestamp,
                            acceleration,
                            angular_velocity,
                            euler_angles,
                        },
                    ) => common::events::DeviceEventKind::AccelerometerEvent(
                        common::events::AccelerometerEvent {
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
                    ) => common::events::DeviceEventKind::TrackingEvent(
                        common::events::TrackingEvent {
                            timestamp,
                            aimpoint: nalgebra::Vector2::new(
                                aimpoint.clone().unwrap().x,
                                aimpoint.clone().unwrap().y,
                            ),
                            pose: if let Some(pose) = pose {
                                Some(common::events::Pose {
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
                    ) => {
                        common::events::DeviceEventKind::ImpactEvent(common::events::ImpactEvent {
                            timestamp,
                        })
                    }
                    proto::device_event::DeviceEventOneof::Connect(_) => {
                        common::events::DeviceEventKind::ConnectEvent
                    }
                    proto::device_event::DeviceEventOneof::Disconnect(_) => {
                        common::events::DeviceEventKind::DisconnectEvent
                    }
                    proto::device_event::DeviceEventOneof::ZeroResult(
                        proto::device_event::ZeroResultEvent { success },
                    ) => common::events::DeviceEventKind::ZeroResult(success),
                    proto::device_event::DeviceEventOneof::SaveZeroResult(
                        proto::device_event::SaveZeroResultEvent { success },
                    ) => common::events::DeviceEventKind::SaveZeroResult(success),
                    proto::device_event::DeviceEventOneof::ShotDelayChanged(
                        proto::device_event::ShotDelayChangedEvent { delay_ms },
                    ) => common::events::DeviceEventKind::ShotDelayChangedEvent(delay_ms as u16),
                    proto::device_event::DeviceEventOneof::Packet(
                        proto::device_event::PacketEvent { bytes },
                    ) => {
                        let mut bytes_slice: &[u8] = &bytes;
                        common::events::DeviceEventKind::PacketEvent(
                            ats_usb::packets::vm::Packet::parse(&mut bytes_slice).unwrap(),
                        )
                    }
                },
            )),
        }
    }
}

impl From<proto::ScreenInfoReply> for common::ScreenInfo {
    fn from(value: proto::ScreenInfoReply) -> Self {
        let bounds = value.bounds.unwrap();
        common::ScreenInfo {
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

impl From<common::AccessoryInfo> for proto::AccessoryInfo {
    fn from(value: common::AccessoryInfo) -> Self {
        Self {
            name: value.name,
            ty: proto::AccessoryType::from(value.ty).into(),
            assignment: if let Some(val) = value.assignment {
                val.get()
            } else {
                0
            },
        }
    }
}

impl From<proto::AccessoryInfo> for common::AccessoryInfo {
    fn from(value: proto::AccessoryInfo) -> Self {
        Self {
            name: value.name,
            ty: proto::AccessoryType::try_from(value.ty).unwrap().into(),
            assignment: if value.assignment != 0 {
                NonZero::new(value.assignment)
            } else {
                None
            },
        }
    }
}

impl From<common::AccessoryType> for proto::AccessoryType {
    fn from(value: common::AccessoryType) -> Self {
        match value {
            common::AccessoryType::DryFireMag => proto::AccessoryType::DryFireMag,
        }
    }
}

impl From<proto::AccessoryType> for common::AccessoryType {
    fn from(value: proto::AccessoryType) -> Self {
        match value {
            proto::AccessoryType::DryFireMag => common::AccessoryType::DryFireMag,
        }
    }
}

impl From<common::AccessoryMap> for proto::AccessoryMapReply {
    fn from(map: common::AccessoryMap) -> Self {
        let accessory_map = map
            .into_iter()
            .map(|(k, (accessory, connected))| {
                let mut buf = [0u8; 8];
                buf[..6].copy_from_slice(&k);
                let key = u64::from_le_bytes(buf);
                let accessory: proto::AccessoryInfo = accessory.into();
                (key, proto::AccessoryStatus {
                    accessory: Some(accessory),
                    connected,
                })
            })
            .collect();
        proto::AccessoryMapReply { accessory_map }
    }
}

impl From<proto::AccessoryMapReply> for common::AccessoryMap {
    fn from(value: proto::AccessoryMapReply) -> Self {
        value
            .accessory_map
            .into_iter()
            .map(|(k, v)| {
                let mut buf = [0u8; 8];
                buf[..6].copy_from_slice(&k.to_le_bytes()[..6]);
                let key = buf[..6].try_into().unwrap();
                (key, (v.accessory.unwrap().into(), v.connected))
            })
            .collect()
    }
}
