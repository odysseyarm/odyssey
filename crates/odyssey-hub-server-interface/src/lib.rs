use std::num::NonZero;

mod proto {
    tonic::include_proto!("odyssey.service_interface");
}
use odyssey_hub_common as common;
pub use proto::*;

// -------- Math type conversions --------

impl From<nalgebra::Matrix3x1<f32>> for Matrix3x1 {
    fn from(v: nalgebra::Matrix3x1<f32>) -> Self {
        Self {
            m11: v.x,
            m21: v.y,
            m31: v.z,
        }
    }
}
impl From<Matrix3x1> for nalgebra::Matrix3x1<f32> {
    fn from(v: Matrix3x1) -> Self {
        Self::new(v.m11, v.m21, v.m31)
    }
}

impl From<nalgebra::Matrix3<f32>> for Matrix3x3 {
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
impl From<Matrix3x3> for nalgebra::Matrix3<f32> {
    fn from(m: Matrix3x3) -> Self {
        Self::new(
            m.m11, m.m12, m.m13, m.m21, m.m22, m.m23, m.m31, m.m32, m.m33,
        )
    }
}

impl From<nalgebra::Vector2<f32>> for Vector2 {
    fn from(v: nalgebra::Vector2<f32>) -> Self {
        Self { x: v.x, y: v.y }
    }
}
impl From<Vector2> for nalgebra::Vector2<f32> {
    fn from(v: Vector2) -> Self {
        Self::new(v.x, v.y)
    }
}

impl From<nalgebra::Vector3<f32>> for Vector3 {
    fn from(v: nalgebra::Vector3<f32>) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}
impl From<Vector3> for nalgebra::Vector3<f32> {
    fn from(v: Vector3) -> Self {
        Self::new(v.x, v.y, v.z)
    }
}

// -------- Device conversions --------

impl From<common::device::Transport> for Transport {
    fn from(t: common::device::Transport) -> Self {
        match t {
            common::device::Transport::Usb => Self::Usb,
            common::device::Transport::UsbMux => Self::UsbMux,
            common::device::Transport::UdpMux => Self::UdpMux,
        }
    }
}
impl From<Transport> for common::device::Transport {
    fn from(t: Transport) -> Self {
        match t {
            Transport::Usb => Self::Usb,
            Transport::UsbMux => Self::UsbMux,
            Transport::UdpMux => Self::UdpMux,
        }
    }
}

impl From<common::device::Device> for Device {
    fn from(d: common::device::Device) -> Self {
        Self {
            uuid: d.uuid.to_vec(),
            transport: Transport::from(d.transport) as i32,
            firmware_version: if d.firmware_version == [0, 0, 0] {
                None
            } else {
                Some(FirmwareVersion {
                    major: u32::from(d.firmware_version[0]),
                    minor: u32::from(d.firmware_version[1]),
                    patch: u32::from(d.firmware_version[2]),
                })
            },
            capabilities: u32::from(d.capabilities.bits()),
            events_transport: match d.events_transport {
                common::device::EventsTransport::Wired => EventsTransport::Wired as i32,
                common::device::EventsTransport::Bluetooth => EventsTransport::Bluetooth as i32,
            },
            events_connected: d.events_connected,
            product_id: u32::from(d.product_id),
            name: d.name().to_string(),
            netcore_release: u32::from(d.netcore_release),
        }
    }
}
impl From<Device> for common::device::Device {
    fn from(d: Device) -> Self {
        let mut uuid = [0u8; 6];
        if d.uuid.len() == 6 {
            uuid.copy_from_slice(&d.uuid);
        } else if !d.uuid.is_empty() {
            // If we have some bytes, take first 6 (or pad)
            let len = std::cmp::min(6, d.uuid.len());
            uuid[..len].copy_from_slice(&d.uuid[..len]);
        }
        Self {
            uuid,
            transport: Transport::try_from(d.transport).unwrap().into(),
            capabilities: common::device::DeviceCapabilities::new(d.capabilities as u8),
            firmware_version: d
                .firmware_version
                .map(|v| [v.major as u16, v.minor as u16, v.patch as u16])
                .unwrap_or([0, 0, 0]),
            events_transport: match EventsTransport::try_from(d.events_transport).unwrap() {
                EventsTransport::Wired => common::device::EventsTransport::Wired,
                EventsTransport::Bluetooth => common::device::EventsTransport::Bluetooth,
            },
            events_connected: d.events_connected,
            product_id: d.product_id as u16,
            name: Self::name_bytes(&d.name),
            netcore_release: u16::try_from(d.netcore_release).unwrap_or(0),
        }
    }
}

// -------- Events --------

impl From<common::events::Event> for Event {
    fn from(value: common::events::Event) -> Self {
        match value {
            common::events::Event::DeviceEvent(common::events::DeviceEvent(device, kind)) => {
                let device_pb: Device = device.into();
                let oneof = match kind {
                    common::events::DeviceEventKind::AccelerometerEvent(e) => {
                        device_event::DeviceEventOneof::Accelerometer(
                            device_event::AccelerometerEvent {
                                timestamp: e.timestamp,
                                acceleration: Some(Vector3::from(e.accel)),
                                angular_velocity: Some(Vector3::from(e.gyro)),
                                euler_angles: Some(Vector3::from(e.euler_angles)),
                            },
                        )
                    }
                    common::events::DeviceEventKind::TrackingEvent(e) => {
                        device_event::DeviceEventOneof::Tracking(device_event::TrackingEvent {
                            timestamp: e.timestamp,
                            aimpoint: Some(Vector2::from(e.aimpoint)),
                            pose: Some(Pose {
                                rotation: Some(Matrix3x3::from(e.pose.rotation)),
                                translation: Some(Matrix3x1::from(e.pose.translation)),
                            }),
                            distance: e.distance,
                            screen_id: e.screen_id,
                        })
                    }
                    common::events::DeviceEventKind::ImpactEvent(e) => {
                        device_event::DeviceEventOneof::Impact(device_event::ImpactEvent {
                            timestamp: e.timestamp,
                        })
                    }
                    common::events::DeviceEventKind::PacketEvent(pkt) => {
                        device_event::DeviceEventOneof::Packet(device_event::PacketEvent {
                            bytes: postcard::to_allocvec(&pkt).expect("postcard serialize Packet"),
                        })
                    }
                    common::events::DeviceEventKind::ZeroResult(success) => {
                        device_event::DeviceEventOneof::ZeroResult(device_event::ZeroResultEvent {
                            success,
                        })
                    }
                    common::events::DeviceEventKind::SaveZeroResult(success) => {
                        device_event::DeviceEventOneof::SaveZeroResult(
                            device_event::SaveZeroResultEvent { success },
                        )
                    }
                    common::events::DeviceEventKind::CapabilitiesChanged => {
                        device_event::DeviceEventOneof::CapabilitiesChanged(
                            device_event::CapabilitiesChangedEvent {},
                        )
                    }
                    common::events::DeviceEventKind::PairingResult {
                        success,
                        paired_address,
                        error,
                        paired_name,
                    } => device_event::DeviceEventOneof::PairingResult(
                        device_event::PairingResultEvent {
                            success,
                            paired_address: paired_address.to_vec(),
                            error,
                            paired_name,
                        },
                    ),
                    common::events::DeviceEventKind::BatteryEvent(e) => {
                        device_event::DeviceEventOneof::Battery(device_event::BatteryEvent {
                            percent: u32::from(e.percent),
                            charging: e.charging,
                        })
                    }
                };
                Self {
                    event_oneof: Some(event::EventOneof::Device(DeviceEvent {
                        device: Some(device_pb),
                        device_event_oneof: Some(oneof),
                    })),
                }
            }
            common::events::Event::DonglePairingResult {
                dongle_id,
                success,
                paired_address,
                error,
                paired_name,
            } => Self {
                event_oneof: Some(event::EventOneof::DonglePairingResult(
                    DonglePairingResultEvent {
                        dongle_id,
                        success,
                        paired_address: paired_address.to_vec(),
                        error,
                        paired_name,
                    },
                )),
            },
        }
    }
}

impl TryFrom<Event> for common::events::Event {
    type Error = ();

    fn try_from(event: Event) -> Result<Self, ()> {
        match event.event_oneof.ok_or(())? {
            event::EventOneof::DonglePairingResult(e) => {
                let mut paired_address = [0u8; 6];
                let len = e.paired_address.len().min(6);
                paired_address[..len].copy_from_slice(&e.paired_address[..len]);
                Ok(Self::DonglePairingResult {
                    dongle_id: e.dongle_id,
                    success: e.success,
                    paired_address,
                    error: e.error,
                    paired_name: e.paired_name,
                })
            }
            event::EventOneof::Device(d) => {
                let device: common::device::Device = match d.device {
                    Some(dev) => dev.into(),
                    None => return Err(()),
                };
                let kind = match d.device_event_oneof.ok_or(())? {
                    device_event::DeviceEventOneof::Accelerometer(e) => {
                        common::events::DeviceEventKind::AccelerometerEvent(
                            common::events::AccelerometerEvent {
                                timestamp: e.timestamp,
                                accel: e.acceleration.expect("acceleration").into(),
                                gyro: e.angular_velocity.expect("angular_velocity").into(),
                                euler_angles: e.euler_angles.expect("euler_angles").into(),
                            },
                        )
                    }
                    device_event::DeviceEventOneof::Tracking(e) => {
                        common::events::DeviceEventKind::TrackingEvent(
                            common::events::TrackingEvent {
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
                            },
                        )
                    }
                    device_event::DeviceEventOneof::Impact(e) => {
                        common::events::DeviceEventKind::ImpactEvent(common::events::ImpactEvent {
                            timestamp: e.timestamp,
                        })
                    }
                    device_event::DeviceEventOneof::Packet(device_event::PacketEvent { bytes }) => {
                        let pkt: odyssey_usb::packets::vm::Packet =
                            postcard::from_bytes(&bytes).expect("postcard deserialize Packet");
                        common::events::DeviceEventKind::PacketEvent(pkt)
                    }
                    device_event::DeviceEventOneof::ZeroResult(e) => {
                        common::events::DeviceEventKind::ZeroResult(e.success)
                    }
                    device_event::DeviceEventOneof::SaveZeroResult(e) => {
                        common::events::DeviceEventKind::SaveZeroResult(e.success)
                    }
                    device_event::DeviceEventOneof::CapabilitiesChanged(_) => {
                        common::events::DeviceEventKind::CapabilitiesChanged
                    }
                    device_event::DeviceEventOneof::PairingResult(e) => {
                        let mut paired_address = [0u8; 6];
                        let len = e.paired_address.len().min(6);
                        paired_address[..len].copy_from_slice(&e.paired_address[..len]);
                        common::events::DeviceEventKind::PairingResult {
                            success: e.success,
                            paired_address,
                            error: e.error,
                            paired_name: e.paired_name,
                        }
                    }
                    device_event::DeviceEventOneof::Battery(e) => {
                        common::events::DeviceEventKind::BatteryEvent(
                            common::events::BatteryEvent {
                                percent: e.percent as u8,
                                charging: e.charging,
                            },
                        )
                    }
                };
                Ok(Self::DeviceEvent(common::events::DeviceEvent(device, kind)))
            }
        }
    }
}

// -------- Accessory conversions (unchanged) --------

impl From<common::ScreenInfo> for ScreenInfoReply {
    fn from(v: common::ScreenInfo) -> Self {
        let bounds = ScreenBounds {
            tl: Some(Vector2::from(v.bounds[0])),
            tr: Some(Vector2::from(v.bounds[1])),
            bl: Some(Vector2::from(v.bounds[2])),
            br: Some(Vector2::from(v.bounds[3])),
        };
        Self {
            id: u32::from(v.id),
            bounds: Some(bounds),
        }
    }
}
impl From<ScreenInfoReply> for common::ScreenInfo {
    fn from(v: ScreenInfoReply) -> Self {
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

impl From<common::accessory::AccessoryFeature> for AccessoryFeature {
    fn from(v: common::accessory::AccessoryFeature) -> Self {
        match v {
            common::accessory::AccessoryFeature::Impact => Self::Impact,
        }
    }
}
impl From<AccessoryFeature> for common::accessory::AccessoryFeature {
    fn from(v: AccessoryFeature) -> Self {
        match v {
            AccessoryFeature::Impact => Self::Impact,
        }
    }
}
impl From<common::accessory::AccessoryType> for AccessoryType {
    fn from(v: common::accessory::AccessoryType) -> Self {
        match v {
            common::accessory::AccessoryType::DryFireMag => Self::DryFireMag,
            common::accessory::AccessoryType::MantisX => Self::MantisX,
        }
    }
}
impl From<AccessoryType> for common::accessory::AccessoryType {
    fn from(v: AccessoryType) -> Self {
        match v {
            AccessoryType::DryFireMag => Self::DryFireMag,
            AccessoryType::MantisX => Self::MantisX,
        }
    }
}

impl From<common::accessory::AccessoryInfo> for AccessoryInfo {
    fn from(v: common::accessory::AccessoryInfo) -> Self {
        Self {
            name: v.name,
            ty: AccessoryType::from(v.ty) as i32,
            assignment: v.assignment.map(NonZero::get).unwrap_or(0),
            features: v
                .features
                .into_iter()
                .map(|f| AccessoryFeature::from(f) as i32)
                .collect(),
        }
    }
}
impl From<AccessoryInfo> for common::accessory::AccessoryInfo {
    fn from(v: AccessoryInfo) -> Self {
        Self {
            name: v.name,
            ty: AccessoryType::try_from(v.ty).unwrap().into(),
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
                        AccessoryFeature::try_from(f).unwrap(),
                    )
                })
                .collect(),
        }
    }
}

impl From<common::accessory::AccessoryMap> for AccessoryMapReply {
    fn from(map: common::accessory::AccessoryMap) -> Self {
        let mut accessory_map = std::collections::HashMap::new();
        for (k6, (info, connected)) in map {
            let mut buf = [0u8; 8];
            buf[..6].copy_from_slice(&k6);
            let key64 = u64::from_le_bytes(buf);
            accessory_map.insert(
                key64,
                AccessoryStatus {
                    accessory: Some(info.into()),
                    connected,
                },
            );
        }
        Self { accessory_map }
    }
}
impl From<AccessoryMapReply> for common::accessory::AccessoryMap {
    fn from(v: AccessoryMapReply) -> Self {
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

impl From<AccessoryInfoMap> for common::accessory::AccessoryInfoMap {
    fn from(v: AccessoryInfoMap) -> Self {
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
impl From<common::accessory::AccessoryInfoMap> for AccessoryInfoMap {
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
