use odyssey_hub_common as common;
use odyssey_usb::packets::vm::VendorData;

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

#[derive(uniffi::Record, Clone)]
pub struct Device {
    pub uuid: Vec<u8>,
    pub transport: common::device::Transport,
    pub capabilities: u8,
    pub firmware_version: Option<Vec<u16>>,
    pub events_transport: common::device::EventsTransport,
    pub events_connected: bool,
    pub product_id: Option<u16>,
    pub name: String,
}

#[derive(uniffi::Enum, Clone)]
pub enum Event {
    AccessoryEvent(AccessoryEvent),
    DeviceEvent(DeviceEvent),
    DonglePairingResult(DonglePairingResult),
}

#[derive(uniffi::Record, Clone)]
pub struct DonglePairingResult {
    pub dongle_id: String,
    pub success: bool,
    pub paired_address: Vec<u8>,
    pub paired_name: String,
    pub error: String,
}

#[derive(uniffi::Record, Clone)]
pub struct DeviceEvent {
    device: Device,
    kind: DeviceEventKind,
}

#[derive(uniffi::Enum, Clone)]
pub enum DeviceEventKind {
    AccelerometerEvent(AccelerometerEvent),
    TrackingEvent(TrackingEvent),
    ImpactEvent(ImpactEvent),
    ZeroResult(bool),
    SaveZeroResult(bool),
    PacketEvent(PacketEvent),
    CapabilitiesChanged,
    PairingResult(PairingResult),
    BatteryEvent(BatteryEvent),
}

#[derive(uniffi::Record, Clone)]
pub struct PairingResult {
    pub success: bool,
    pub paired_address: Vec<u8>,
    pub paired_name: String,
    pub error: String,
}

#[derive(uniffi::Record, Clone)]
pub struct BatteryEvent {
    pub percent: u8,
    pub charging: bool,
}

#[derive(uniffi::Record, Clone)]
pub struct AccessoryEvent {
    pub info: AccessoryInfo,
    pub kind: AccessoryEventKind,
}

#[derive(uniffi::Enum, Clone)]
pub enum AccessoryEventKind {
    Connect(Option<u64>),
    Disconnect,
    AssignmentChange(Option<u64>),
}

#[derive(uniffi::Record, Clone)]
pub struct AccelerometerEvent {
    pub timestamp: u32,
    pub accel: crate::funny::Vector3f32,
    pub gyro: crate::funny::Vector3f32,
    pub euler_angles: crate::funny::Vector3f32,
}

#[derive(uniffi::Record, Clone)]
pub struct TrackingEvent {
    pub timestamp: u32,
    pub aimpoint: crate::funny::Vector2f32,
    pub pose: Pose,
    pub distance: f32,
    pub screen_id: u32,
}

#[derive(uniffi::Record, Clone)]
pub struct ImpactEvent {
    pub timestamp: u32,
}

#[derive(uniffi::Record, Clone)]
pub struct PacketEvent {
    pub ty: u8,
    pub data: PacketData,
}

#[derive(uniffi::Enum, Clone)]
pub enum PacketData {
    Unsupported,
    VendorEvent(VendorEventPacketData),
}

#[derive(uniffi::Record, Clone)]
pub struct VendorEventPacketData {
    pub len: u8,
    pub data: Vec<u8>,
}

#[derive(uniffi::Record, Clone, Default)]
pub struct Pose {
    pub rotation: crate::funny::Matrix3f32,
    pub translation: crate::funny::Matrix3x1f32,
}

#[derive(uniffi::Record, Clone)]
pub struct ScreenInfo {
    pub id: u8,
    pub tl: crate::funny::Vector2f32,
    pub tr: crate::funny::Vector2f32,
    pub bl: crate::funny::Vector2f32,
    pub br: crate::funny::Vector2f32,
}

#[derive(uniffi::Enum, Clone)]
pub enum AccessoryType {
    DryFireMag,
    MantisX,
    MarkerControlBoard,
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum AccessoryRadio {
    Native { id: String },
    Dongle { id: String },
}

#[derive(uniffi::Record, Clone)]
pub struct AccessoryInfo {
    pub name: String,
    pub ty: AccessoryType,
    pub assignment: Option<u64>,
    /// Zero-based screen index; `None` leaves a marker board unassigned.
    #[uniffi(default = None)]
    pub screen_id: Option<u8>,
    /// `None` selects the system Bluetooth radio.
    #[uniffi(default = None)]
    pub radio: Option<AccessoryRadio>,
}

#[derive(uniffi::Record, Clone)]
pub struct AccessoryStatus {
    pub info: AccessoryInfo,
    pub connected: bool,
}

#[derive(uniffi::Record, Clone)]
pub struct AccessoryMapEntry {
    pub key: u64,
    pub status: AccessoryStatus,
}

#[derive(uniffi::Record, Clone)]
pub struct AccessoryInfoMapEntry {
    pub key: u64,
    pub info: AccessoryInfo,
}

impl_from_simple!(common::events::AccelerometerEvent => AccelerometerEvent, timestamp, accel, gyro, euler_angles);
impl_from_simple!(common::events::ImpactEvent => ImpactEvent, timestamp);
impl_from_simple!(common::events::Pose => Pose, rotation, translation);
impl_from_simple!(Pose => common::events::Pose, rotation, translation);

impl From<common::accessory::AccessoryType> for AccessoryType {
    fn from(accessory_type: common::accessory::AccessoryType) -> Self {
        match accessory_type {
            common::accessory::AccessoryType::DryFireMag => Self::DryFireMag,
            common::accessory::AccessoryType::MantisX => Self::MantisX,
            common::accessory::AccessoryType::MarkerControlBoard => Self::MarkerControlBoard,
        }
    }
}

impl From<common::accessory::AccessoryInfo> for AccessoryInfo {
    fn from(info: common::accessory::AccessoryInfo) -> Self {
        AccessoryInfo {
            name: info.name,
            ty: info.ty.into(),
            assignment: info.assignment.map(|nz| nz.get()),
            screen_id: info.screen_id,
            radio: match info.radio {
                common::accessory::AccessoryRadio::System => None,
                common::accessory::AccessoryRadio::Native { id } => {
                    Some(AccessoryRadio::Native { id })
                }
                common::accessory::AccessoryRadio::Dongle { id } => {
                    Some(AccessoryRadio::Dongle { id })
                }
            },
        }
    }
}

impl From<AccessoryType> for common::accessory::AccessoryType {
    fn from(accessory_type: AccessoryType) -> Self {
        match accessory_type {
            AccessoryType::DryFireMag => common::accessory::AccessoryType::DryFireMag,
            AccessoryType::MantisX => common::accessory::AccessoryType::MantisX,
            AccessoryType::MarkerControlBoard => {
                common::accessory::AccessoryType::MarkerControlBoard
            }
        }
    }
}

impl TryFrom<AccessoryInfo> for common::accessory::AccessoryInfo {
    type Error = crate::client::ClientError;

    fn try_from(info: AccessoryInfo) -> Result<Self, Self::Error> {
        if info
            .screen_id
            .is_some_and(|id| id > odyssey_common::MAX_SCREEN_ID)
        {
            return Err(Self::Error::InvalidArgument {
                message: "accessory screen ID is out of range".to_string(),
            });
        }
        let mut result = Self::with_defaults(info.name, info.ty.into());
        result.assignment = info
            .assignment
            .map(|value| {
                std::num::NonZeroU64::new(value).ok_or_else(|| Self::Error::InvalidArgument {
                    message: "accessory assignment must be nonzero when present".to_string(),
                })
            })
            .transpose()?;
        result.screen_id = info.screen_id;
        result.radio = match info.radio {
            None => common::accessory::AccessoryRadio::System,
            Some(AccessoryRadio::Native { id }) => common::accessory::AccessoryRadio::Native { id },
            Some(AccessoryRadio::Dongle { id }) => common::accessory::AccessoryRadio::Dongle { id },
        };
        if let common::accessory::AccessoryRadio::Native { id }
        | common::accessory::AccessoryRadio::Dongle { id } = &result.radio
            && id.trim().is_empty()
        {
            return Err(Self::Error::InvalidArgument {
                message: "accessory radio.id must not be blank".to_string(),
            });
        }
        Ok(result)
    }
}

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

impl From<common::events::TrackingEvent> for TrackingEvent {
    fn from(e: common::events::TrackingEvent) -> Self {
        Self {
            timestamp: e.timestamp,
            aimpoint: e.aimpoint.into(),
            pose: e.pose.into(),
            distance: e.distance,
            screen_id: e.screen_id,
        }
    }
}

impl From<TrackingEvent> for common::events::TrackingEvent {
    fn from(e: TrackingEvent) -> Self {
        Self {
            timestamp: e.timestamp,
            aimpoint: e.aimpoint.into(),
            pose: e.pose.into(),
            distance: e.distance,
            screen_id: e.screen_id,
        }
    }
}

impl From<common::device::Device> for Device {
    fn from(device: common::device::Device) -> Self {
        Self {
            uuid: device.uuid.into(),
            transport: device.transport,
            capabilities: device.capabilities.bits(),
            firmware_version: if device.firmware_version == [0, 0, 0] {
                None
            } else {
                Some(device.firmware_version.to_vec())
            },
            events_transport: device.events_transport,
            events_connected: device.events_connected,
            product_id: if device.product_id == 0 {
                None
            } else {
                Some(device.product_id)
            },
            name: device.name().to_string(),
        }
    }
}

impl TryFrom<Device> for common::device::Device {
    type Error = crate::client::ClientError;

    fn try_from(device: Device) -> Result<Self, Self::Error> {
        let uuid = device
            .uuid
            .try_into()
            .map_err(|_| Self::Error::InvalidArgument {
                message: "device UUID must contain exactly 6 bytes".to_string(),
            })?;
        let firmware_version = device
            .firmware_version
            .map(|version| {
                version
                    .try_into()
                    .map_err(|_| Self::Error::InvalidArgument {
                        message: "firmware version must contain exactly 3 components".to_string(),
                    })
            })
            .transpose()?
            .unwrap_or([0, 0, 0]);
        Ok(Self {
            uuid,
            transport: device.transport,
            capabilities: common::device::DeviceCapabilities::new(device.capabilities),
            firmware_version,
            events_transport: device.events_transport,
            events_connected: device.events_connected,
            product_id: device.product_id.unwrap_or(0),
            name: Self::name_bytes(&device.name),
            // The bindings do not carry the net core release; the server resolves a
            // device by uuid, so a reconstructed device does not need it.
            netcore_release: 0,
        })
    }
}

impl From<odyssey_usb::packets::vm::PacketData> for PacketData {
    fn from(packet_data: odyssey_usb::packets::vm::PacketData) -> Self {
        match packet_data {
            odyssey_usb::packets::vm::PacketData::Vendor(_, VendorData { len, data }) => {
                PacketData::VendorEvent(VendorEventPacketData {
                    len,
                    data: data.to_vec(),
                })
            }
            _ => PacketData::Unsupported,
        }
    }
}

impl From<common::events::Event> for Event {
    fn from(event: common::events::Event) -> Self {
        match event {
            common::events::Event::DeviceEvent(common::events::DeviceEvent(d, evt)) => {
                Event::DeviceEvent(DeviceEvent {
                    device: d.into(),
                    kind: match evt {
                        common::events::DeviceEventKind::AccelerometerEvent(e) => {
                            DeviceEventKind::AccelerometerEvent(e.into())
                        }
                        common::events::DeviceEventKind::TrackingEvent(e) => {
                            DeviceEventKind::TrackingEvent(TrackingEvent {
                                timestamp: e.timestamp,
                                aimpoint: e.aimpoint.into(),
                                pose: e.pose.into(),
                                distance: e.distance,
                                screen_id: e.screen_id,
                            })
                        }
                        common::events::DeviceEventKind::ImpactEvent(e) => {
                            DeviceEventKind::ImpactEvent(e.into())
                        }
                        common::events::DeviceEventKind::ZeroResult(b) => {
                            DeviceEventKind::ZeroResult(b)
                        }
                        common::events::DeviceEventKind::SaveZeroResult(b) => {
                            DeviceEventKind::SaveZeroResult(b)
                        }
                        common::events::DeviceEventKind::PacketEvent(p) => {
                            DeviceEventKind::PacketEvent(PacketEvent {
                                ty: p.ty().into(),
                                data: p.data.into(),
                            })
                        }
                        common::events::DeviceEventKind::CapabilitiesChanged => {
                            DeviceEventKind::CapabilitiesChanged
                        }
                        common::events::DeviceEventKind::PairingResult {
                            success,
                            paired_address,
                            paired_name,
                            error,
                        } => DeviceEventKind::PairingResult(PairingResult {
                            success,
                            paired_address: paired_address.to_vec(),
                            paired_name,
                            error,
                        }),
                        common::events::DeviceEventKind::BatteryEvent(e) => {
                            DeviceEventKind::BatteryEvent(BatteryEvent {
                                percent: e.percent,
                                charging: e.charging,
                            })
                        }
                    },
                })
            }
            common::events::Event::DonglePairingResult {
                dongle_id,
                success,
                paired_address,
                paired_name,
                error,
            } => Event::DonglePairingResult(DonglePairingResult {
                dongle_id,
                success,
                paired_address: paired_address.to_vec(),
                paired_name,
                error,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ClientError;

    fn device() -> Device {
        Device {
            uuid: vec![1, 2, 3, 4, 5, 6],
            transport: common::device::Transport::Usb,
            capabilities: 0,
            firmware_version: Some(vec![1, 2, 3]),
            events_transport: common::device::EventsTransport::Wired,
            events_connected: false,
            product_id: None,
            name: String::new(),
        }
    }

    #[test]
    fn device_vector_lengths_are_validated_at_the_boundary() {
        for length in [0, 5, 7] {
            let mut device = device();
            device.uuid.resize(length, 0);
            assert!(matches!(
                common::device::Device::try_from(device),
                Err(ClientError::InvalidArgument { .. }),
            ));
        }
        for length in [0, 2, 4] {
            let mut device = device();
            device.firmware_version = Some(vec![0; length]);
            assert!(matches!(
                common::device::Device::try_from(device),
                Err(ClientError::InvalidArgument { .. }),
            ));
        }
        let converted = common::device::Device::try_from(device()).expect("valid device");
        assert_eq!(converted.uuid, [1, 2, 3, 4, 5, 6]);
        assert_eq!(converted.firmware_version, [1, 2, 3]);
    }

    #[test]
    fn zero_is_not_silently_converted_to_an_unassigned_accessory() {
        let result = common::accessory::AccessoryInfo::try_from(AccessoryInfo {
            name: "accessory".to_string(),
            ty: AccessoryType::DryFireMag,
            assignment: Some(0),
            screen_id: None,
            radio: None,
        });
        assert!(matches!(result, Err(ClientError::InvalidArgument { .. })));
    }

    #[test]
    fn marker_screen_assignments_preserve_six_slots_and_reject_overflow() {
        for screen_id in 0..=odyssey_common::MAX_SCREEN_ID {
            let converted = common::accessory::AccessoryInfo::try_from(AccessoryInfo {
                name: "markers".to_string(),
                ty: AccessoryType::MarkerControlBoard,
                assignment: None,
                screen_id: Some(screen_id),
                radio: None,
            })
            .expect("supported screen");
            assert_eq!(converted.screen_id, Some(screen_id));
        }
        for screen_id in [odyssey_common::MAX_SCREEN_ID + 1, u8::MAX] {
            let converted = common::accessory::AccessoryInfo::try_from(AccessoryInfo {
                name: "markers".to_string(),
                ty: AccessoryType::MarkerControlBoard,
                assignment: None,
                screen_id: Some(screen_id),
                radio: None,
            });
            assert!(matches!(
                converted,
                Err(ClientError::InvalidArgument { .. })
            ));
        }
    }

    #[test]
    fn radio_ids_reject_empty_and_whitespace() {
        for id in ["", " \t"] {
            for radio in [
                AccessoryRadio::Native { id: id.to_string() },
                AccessoryRadio::Dongle { id: id.to_string() },
            ] {
                let result = common::accessory::AccessoryInfo::try_from(AccessoryInfo {
                    name: "markers".to_string(),
                    ty: AccessoryType::MarkerControlBoard,
                    assignment: None,
                    screen_id: Some(5),
                    radio: Some(radio),
                });
                assert!(
                    matches!(result, Err(ClientError::InvalidArgument { .. })),
                    "{id:?}"
                );
            }
        }
    }
}
