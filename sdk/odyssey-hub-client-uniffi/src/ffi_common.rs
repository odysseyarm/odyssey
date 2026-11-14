use odyssey_hub_common as common;
use odyssey_usb::packets::vm::VendorData;

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
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

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct Device {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub uuid: Vec<u8>,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub transport: common::device::Transport,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub capabilities: u8,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub firmware_version: Option<Vec<u32>>,
    /// Since: UniFFI SDK (unreleased).
    pub firmware_version_string: Option<String>,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub events_transport: common::device::EventsTransport,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub events_connected: bool,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub product_id: Option<u16>,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub name: String,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Enum, Clone, Debug)]
pub enum Event {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    AccessoryEvent(AccessoryEvent),
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    DeviceEvent(DeviceEvent),
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    DonglePairingResult(DonglePairingResult),
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct DonglePairingResult {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub dongle_id: String,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub success: bool,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub paired_address: Vec<u8>,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub paired_name: String,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub error: String,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct DeviceEvent {
    device: Device,
    kind: DeviceEventKind,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Enum, Clone, Debug)]
pub enum DeviceEventKind {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    AccelerometerEvent(AccelerometerEvent),
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    TrackingEvent(TrackingEvent),
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    ImpactEvent(ImpactEvent),
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    ZeroResult(bool),
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    SaveZeroResult(bool),
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    PacketEvent(PacketEvent),
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    CapabilitiesChanged,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    PairingResult(PairingResult),
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    BatteryEvent(BatteryEvent),
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct PairingResult {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub success: bool,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub paired_address: Vec<u8>,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub paired_name: String,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub error: String,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct BatteryEvent {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub percent: u8,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub charging: bool,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct AccessoryEvent {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub info: AccessoryInfo,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub kind: AccessoryEventKind,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Enum, Clone, Debug)]
pub enum AccessoryEventKind {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    Connect(Option<u64>),
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    Disconnect,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    AssignmentChange(Option<u64>),
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct AccelerometerEvent {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub timestamp: u32,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub accel: crate::funny::Vector3f32,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub gyro: crate::funny::Vector3f32,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub euler_angles: crate::funny::Vector3f32,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct TrackingEvent {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub timestamp: u32,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub aimpoint: crate::funny::Vector2f32,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub pose: Pose,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub distance: f32,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub screen_id: u32,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct ImpactEvent {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub timestamp: u32,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct PacketEvent {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub ty: u8,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub data: PacketData,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Enum, Clone, Debug)]
pub enum PacketData {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    Unsupported,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    VendorEvent(VendorEventPacketData),
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct VendorEventPacketData {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub len: u8,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub data: Vec<u8>,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug, Default)]
pub struct Pose {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub rotation: crate::funny::Matrix3f32,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub translation: crate::funny::Matrix3x1f32,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct ScreenInfo {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub id: u8,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub tl: crate::funny::Vector2f32,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub tr: crate::funny::Vector2f32,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub bl: crate::funny::Vector2f32,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub br: crate::funny::Vector2f32,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Enum, Clone, Debug)]
pub enum AccessoryType {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    DryFireMag,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    MantisX,
    /// Since: UniFFI SDK (unreleased).
    MarkerControlBoard,
}

/// Since: UniFFI SDK (unreleased).
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum AccessoryRadio {
    /// Since: UniFFI SDK (unreleased).
    Native {
        #[doc = "Since: UniFFI SDK (unreleased)."]
        id: String,
    },
    /// Since: UniFFI SDK (unreleased).
    Dongle {
        #[doc = "Since: UniFFI SDK (unreleased)."]
        id: String,
    },
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct AccessoryInfo {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub name: String,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub ty: AccessoryType,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub assignment: Option<u64>,
    /// Zero-based screen index; `None` leaves a marker board unassigned.
    ///
    /// Since: UniFFI SDK (unreleased).
    #[uniffi(default = None)]
    pub screen_id: Option<u8>,
    /// `None` selects the system Bluetooth radio.
    ///
    /// Since: UniFFI SDK (unreleased).
    #[uniffi(default = None)]
    pub radio: Option<AccessoryRadio>,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct AccessoryStatus {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub info: AccessoryInfo,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub connected: bool,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct AccessoryMapEntry {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub key: u64,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub status: AccessoryStatus,
}

/// Since: UniFFI SDK 0.2.1 (earliest tagged release).
#[derive(uniffi::Record, Clone, Debug)]
pub struct AccessoryInfoMapEntry {
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
    pub key: u64,
    /// Since: UniFFI SDK 0.2.1 (earliest tagged release).
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
        Self {
            name: info.name,
            ty: info.ty.into(),
            assignment: info.assignment.map(std::num::NonZero::get),
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
            AccessoryType::DryFireMag => Self::DryFireMag,
            AccessoryType::MantisX => Self::MantisX,
            AccessoryType::MarkerControlBoard => {
                Self::MarkerControlBoard
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
        Self {
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
            firmware_version_string: device
                .firmware_identity
                .known()
                .map(|version| version.to_string()),
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
            .map_err(|_length| Self::Error::InvalidArgument {
                message: "device UUID must contain exactly 6 bytes".to_string(),
            })?;
        let firmware_version = device
            .firmware_version
            .map(|version| {
                version
                    .try_into()
                    .map_err(|_length| Self::Error::InvalidArgument {
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
            firmware_identity: device
                .firmware_version_string
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_parse_error| Self::Error::InvalidArgument {
                    message: "firmware version is not an Odyssey release".to_owned(),
                })?
                .unwrap_or_default(),
            events_transport: device.events_transport,
            events_connected: device.events_connected,
            product_id: device.product_id.unwrap_or(0),
            name: Self::name_bytes(&device.name),
            netcore_release: 0,
        })
    }
}
impl From<odyssey_usb::packets::vm::PacketData> for PacketData {
    fn from(packet_data: odyssey_usb::packets::vm::PacketData) -> Self {
        match packet_data {
            odyssey_usb::packets::vm::PacketData::Vendor(_, VendorData { len, data }) => {
                Self::VendorEvent(VendorEventPacketData {
                    len,
                    data: data.to_vec(),
                })
            }
            _ => Self::Unsupported,
        }
    }
}

impl From<common::events::Event> for Event {
    fn from(event: common::events::Event) -> Self {
        match event {
            common::events::Event::DeviceEvent(common::events::DeviceEvent(d, evt)) => {
                Self::DeviceEvent(DeviceEvent {
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
            } => Self::DonglePairingResult(DonglePairingResult {
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
            firmware_version_string: None,
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
