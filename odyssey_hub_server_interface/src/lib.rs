use std::num::NonZero;

mod proto {
    tonic::include_proto!("odyssey.service_interface");
}
pub use proto::*;

use odyssey_hub_common as common;

// -------- Math type conversions --------

impl From<nalgebra::Matrix3x1<f32>> for proto::Matrix3x1 {
    fn from(v: nalgebra::Matrix3x1<f32>) -> Self {
        Self {
            m11: v.x,
            m21: v.y,
            m31: v.z,
        }
    }
}
impl From<proto::Matrix3x1> for nalgebra::Matrix3x1<f32> {
    fn from(v: proto::Matrix3x1) -> Self {
        nalgebra::Matrix3x1::new(v.m11, v.m21, v.m31)
    }
}

impl From<nalgebra::Matrix3<f32>> for proto::Matrix3x3 {
    fn from(m: nalgebra::Matrix3<f32>) -> Self {
        Self {
            m11: m.m11,
            m12: m.m12,
            m13: m.m13,
            m21: m.m21,
            m22: m.m22,
            m23: m.m23,
            m31: m.m31,
            m32: m.m32,
            m33: m.m33,
        }
    }
}
impl From<proto::Matrix3x3> for nalgebra::Matrix3<f32> {
    fn from(m: proto::Matrix3x3) -> Self {
        nalgebra::Matrix3::new(
            m.m11, m.m12, m.m13, m.m21, m.m22, m.m23, m.m31, m.m32, m.m33,
        )
    }
}

impl From<nalgebra::Vector2<f32>> for proto::Vector2 {
    fn from(v: nalgebra::Vector2<f32>) -> Self {
        Self { x: v.x, y: v.y }
    }
}
impl From<proto::Vector2> for nalgebra::Vector2<f32> {
    fn from(v: proto::Vector2) -> Self {
        nalgebra::Vector2::new(v.x, v.y)
    }
}

impl From<nalgebra::Vector3<f32>> for proto::Vector3 {
    fn from(v: nalgebra::Vector3<f32>) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}
impl From<proto::Vector3> for nalgebra::Vector3<f32> {
    fn from(v: proto::Vector3) -> Self {
        nalgebra::Vector3::new(v.x, v.y, v.z)
    }
}

// -------- Device conversions --------

impl From<common::device::Transport> for proto::Transport {
    fn from(t: common::device::Transport) -> Self {
        match t {
            common::device::Transport::Usb => Self::Usb,
            common::device::Transport::UsbMux => Self::UsbMux,
            common::device::Transport::UdpMux => Self::UdpMux,
        }
    }
}
impl From<proto::Transport> for common::device::Transport {
    fn from(t: proto::Transport) -> Self {
        match t {
            proto::Transport::Usb => common::device::Transport::Usb,
            proto::Transport::UsbMux => common::device::Transport::UsbMux,
            proto::Transport::UdpMux => common::device::Transport::UdpMux,
        }
    }
}

impl From<common::device::Device> for proto::Device {
    fn from(d: common::device::Device) -> Self {
        Self {
            uuid: d.uuid.to_vec(),
            transport: proto::Transport::from(d.transport) as i32,
            firmware_version: if d.firmware_version == [0, 0, 0] {
                None
            } else {
                Some(proto::FirmwareVersion {
                    major: d.firmware_version[0] as u32,
                    minor: d.firmware_version[1] as u32,
                    patch: d.firmware_version[2] as u32,
                })
            },
            capabilities: d.capabilities.bits() as u32,
            events_transport: match d.events_transport {
                common::device::EventsTransport::Wired => proto::EventsTransport::Wired as i32,
                common::device::EventsTransport::Bluetooth => {
                    proto::EventsTransport::Bluetooth as i32
                }
            },
            events_connected: d.events_connected,
            product_id: d.product_id as u32,
        }
    }
}
impl From<proto::Device> for common::device::Device {
    fn from(d: proto::Device) -> Self {
        let mut uuid = [0u8; 6];
        if d.uuid.len() == 6 {
            uuid.copy_from_slice(&d.uuid);
        } else if d.uuid.len() > 0 {
            // If we have some bytes, take first 6 (or pad)
            let len = std::cmp::min(6, d.uuid.len());
            uuid[..len].copy_from_slice(&d.uuid[..len]);
        }
        common::device::Device {
            uuid,
            transport: proto::Transport::try_from(d.transport).unwrap().into(),
            capabilities: common::device::DeviceCapabilities::new(d.capabilities as u8),
            firmware_version: d
                .firmware_version
                .map(|v| [v.major as u16, v.minor as u16, v.patch as u16])
                .unwrap_or([0, 0, 0]),
            events_transport: match proto::EventsTransport::try_from(d.events_transport).unwrap() {
                proto::EventsTransport::Wired => common::device::EventsTransport::Wired,
                proto::EventsTransport::Bluetooth => common::device::EventsTransport::Bluetooth,
            },
            events_connected: d.events_connected,
            product_id: d.product_id as u16,
        }
    }
}

// -------- Events --------

impl From<common::events::Event> for proto::Event {
    fn from(value: common::events::Event) -> Self {
        match value {
            common::events::Event::DeviceEvent(common::events::DeviceEvent(device, kind)) => {
                let device_pb: proto::Device = device.into();
                let oneof = match kind {
                    common::events::DeviceEventKind::AccelerometerEvent(e) => {
                        proto::device_event::DeviceEventOneof::Accelerometer(
                            proto::device_event::AccelerometerEvent {
                                timestamp: e.timestamp,
                                acceleration: Some(proto::Vector3::from(e.accel)),
                                angular_velocity: Some(proto::Vector3::from(e.gyro)),
                                euler_angles: Some(proto::Vector3::from(e.euler_angles)),
                            },
                        )
                    }
                    common::events::DeviceEventKind::TrackingEvent(e) => {
                        proto::device_event::DeviceEventOneof::Tracking(
                            proto::device_event::TrackingEvent {
                                timestamp: e.timestamp,
                                aimpoint: Some(proto::Vector2::from(e.aimpoint)),
                                pose: Some(proto::Pose {
                                    rotation: Some(proto::Matrix3x3::from(e.pose.rotation)),
                                    translation: Some(proto::Matrix3x1::from(e.pose.translation)),
                                }),
                                distance: e.distance,
                                screen_id: e.screen_id,
                            },
                        )
                    }
                    common::events::DeviceEventKind::ImpactEvent(e) => {
                        proto::device_event::DeviceEventOneof::Impact(
                            proto::device_event::ImpactEvent {
                                timestamp: e.timestamp,
                            },
                        )
                    }
                    common::events::DeviceEventKind::PacketEvent(pkt) => {
                        proto::device_event::DeviceEventOneof::Packet(
                            proto::device_event::PacketEvent {
                                bytes: postcard::to_allocvec(&pkt)
                                    .expect("postcard serialize Packet"),
                            },
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
                    common::events::DeviceEventKind::CapabilitiesChanged => {
                        proto::device_event::DeviceEventOneof::CapabilitiesChanged(
                            proto::device_event::CapabilitiesChangedEvent {},
                        )
                    }
                };
                proto::Event {
                    event_oneof: Some(proto::event::EventOneof::Device(proto::DeviceEvent {
                        device: Some(device_pb),
                        device_event_oneof: Some(oneof),
                    })),
                }
            }
        }
    }
}

impl From<proto::Event> for common::events::Event {
    fn from(event: proto::Event) -> Self {
        let Some(proto::event::EventOneof::Device(d)) = event.event_oneof else {
            panic!("proto::Event missing device variant");
        };
        let device: common::device::Device = match d.device {
            Some(dev) => dev.into(),
            None => panic!("proto::DeviceEvent missing device field"),
        };
        let kind = match d
            .device_event_oneof
            .expect("proto::DeviceEvent missing payload")
        {
            proto::device_event::DeviceEventOneof::Accelerometer(e) => {
                common::events::DeviceEventKind::AccelerometerEvent(
                    common::events::AccelerometerEvent {
                        timestamp: e.timestamp,
                        accel: e.acceleration.expect("acceleration").into(),
                        gyro: e.angular_velocity.expect("angular_velocity").into(),
                        euler_angles: e.euler_angles.expect("euler_angles").into(),
                    },
                )
            }
            proto::device_event::DeviceEventOneof::Tracking(e) => {
                common::events::DeviceEventKind::TrackingEvent(common::events::TrackingEvent {
                    timestamp: e.timestamp,
                    aimpoint: e.aimpoint.expect("aimpoint").into(),
                    pose: {
                        let p = e.pose.expect("pose");
                        common::events::Pose {
                            rotation: p.rotation.expect("rotation").into(),
                            translation: p.translation.expect("translation").into(),
                        }
                    },
                    distance: e.distance,
                    screen_id: e.screen_id,
                })
            }
            proto::device_event::DeviceEventOneof::Impact(e) => {
                common::events::DeviceEventKind::ImpactEvent(common::events::ImpactEvent {
                    timestamp: e.timestamp,
                })
            }
            proto::device_event::DeviceEventOneof::Packet(proto::device_event::PacketEvent {
                bytes,
            }) => {
                let pkt: ats_usb::packets::vm::Packet =
                    postcard::from_bytes(&bytes).expect("postcard deserialize Packet");
                common::events::DeviceEventKind::PacketEvent(pkt)
            }
            proto::device_event::DeviceEventOneof::ZeroResult(e) => {
                common::events::DeviceEventKind::ZeroResult(e.success)
            }
            proto::device_event::DeviceEventOneof::SaveZeroResult(e) => {
                common::events::DeviceEventKind::SaveZeroResult(e.success)
            }
            proto::device_event::DeviceEventOneof::CapabilitiesChanged(_) => {
                common::events::DeviceEventKind::CapabilitiesChanged
            }
        };
        common::events::Event::DeviceEvent(common::events::DeviceEvent(device, kind))
    }
}

// -------- Accessory conversions (unchanged) --------

impl From<common::ScreenInfo> for proto::ScreenInfoReply {
    fn from(v: common::ScreenInfo) -> Self {
        let bounds = proto::ScreenBounds {
            tl: Some(proto::Vector2::from(v.bounds[0])),
            tr: Some(proto::Vector2::from(v.bounds[1])),
            bl: Some(proto::Vector2::from(v.bounds[2])),
            br: Some(proto::Vector2::from(v.bounds[3])),
        };
        Self {
            id: v.id as u32,
            bounds: Some(bounds),
        }
    }
}
impl From<proto::ScreenInfoReply> for common::ScreenInfo {
    fn from(v: proto::ScreenInfoReply) -> Self {
        let b = v.bounds.unwrap();
        Self {
            id: v.id as u8,
            bounds: [
                b.tl.unwrap().into(),
                b.tr.unwrap().into(),
                b.bl.unwrap().into(),
                b.br.unwrap().into(),
            ],
        }
    }
}

impl From<common::accessory::AccessoryFeature> for proto::AccessoryFeature {
    fn from(v: common::accessory::AccessoryFeature) -> Self {
        match v {
            common::accessory::AccessoryFeature::Impact => Self::Impact,
        }
    }
}
impl From<proto::AccessoryFeature> for common::accessory::AccessoryFeature {
    fn from(v: proto::AccessoryFeature) -> Self {
        match v {
            proto::AccessoryFeature::Impact => common::accessory::AccessoryFeature::Impact,
        }
    }
}
impl From<common::accessory::AccessoryType> for proto::AccessoryType {
    fn from(v: common::accessory::AccessoryType) -> Self {
        match v {
            common::accessory::AccessoryType::DryFireMag => Self::DryFireMag,
            common::accessory::AccessoryType::BlackbeardX => Self::BlackbeardX,
        }
    }
}
impl From<proto::AccessoryType> for common::accessory::AccessoryType {
    fn from(v: proto::AccessoryType) -> Self {
        match v {
            proto::AccessoryType::DryFireMag => common::accessory::AccessoryType::DryFireMag,
            proto::AccessoryType::BlackbeardX => common::accessory::AccessoryType::BlackbeardX,
        }
    }
}

impl From<common::accessory::AccessoryInfo> for proto::AccessoryInfo {
    fn from(v: common::accessory::AccessoryInfo) -> Self {
        Self {
            name: v.name,
            ty: proto::AccessoryType::from(v.ty) as i32,
            assignment: v.assignment.map(NonZero::get).unwrap_or(0),
            features: v
                .features
                .into_iter()
                .map(|f| proto::AccessoryFeature::from(f) as i32)
                .collect(),
        }
    }
}
impl From<proto::AccessoryInfo> for common::accessory::AccessoryInfo {
    fn from(v: proto::AccessoryInfo) -> Self {
        Self {
            name: v.name,
            ty: proto::AccessoryType::try_from(v.ty).unwrap().into(),
            assignment: if v.assignment != 0 {
                NonZero::new(v.assignment)
            } else {
                None
            },
            features: v
                .features
                .into_iter()
                .map(|f| {
                    common::accessory::AccessoryFeature::from(
                        proto::AccessoryFeature::try_from(f).unwrap(),
                    )
                })
                .collect(),
        }
    }
}

impl From<common::accessory::AccessoryMap> for proto::AccessoryMapReply {
    fn from(map: common::accessory::AccessoryMap) -> Self {
        let mut accessory_map = std::collections::HashMap::new();
        for (k6, (info, connected)) in map {
            let mut buf = [0u8; 8];
            buf[..6].copy_from_slice(&k6);
            let key64 = u64::from_le_bytes(buf);
            accessory_map.insert(
                key64,
                proto::AccessoryStatus {
                    accessory: Some(info.into()),
                    connected,
                },
            );
        }
        Self { accessory_map }
    }
}
impl From<proto::AccessoryMapReply> for common::accessory::AccessoryMap {
    fn from(v: proto::AccessoryMapReply) -> Self {
        v.accessory_map
            .into_iter()
            .map(|(k64, status)| {
                let mut buf = [0u8; 8];
                buf[..6].copy_from_slice(&k64.to_le_bytes()[..6]);
                let k6: [u8; 6] = buf[..6].try_into().unwrap();
                (k6, (status.accessory.unwrap().into(), status.connected))
            })
            .collect()
    }
}

impl From<proto::AccessoryInfoMap> for common::accessory::AccessoryInfoMap {
    fn from(v: proto::AccessoryInfoMap) -> Self {
        v.accessory_info_map
            .into_iter()
            .map(|(k64, info)| {
                let mut buf = [0u8; 8];
                buf[..6].copy_from_slice(&k64.to_le_bytes()[..6]);
                let k6: [u8; 6] = buf[..6].try_into().unwrap();
                (k6, info.into())
            })
            .collect()
    }
}
impl From<common::accessory::AccessoryInfoMap> for proto::AccessoryInfoMap {
    fn from(map: common::accessory::AccessoryInfoMap) -> Self {
        let mut m = std::collections::HashMap::new();
        for (k6, info) in map {
            let mut buf = [0u8; 8];
            buf[..6].copy_from_slice(&k6);
            m.insert(u64::from_le_bytes(buf), info.into());
        }
        Self {
            accessory_info_map: m,
        }
    }
}
