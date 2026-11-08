use std::num::NonZero;

use odyssey_hub_common as common;

use super::hardware_address::{address_from_key, address_to_key};
use crate::{
    AccessoryFeature, AccessoryInfo, AccessoryInfoMap, AccessoryMapReply, AccessoryStatus,
    AccessoryType,
};

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
            common::accessory::AccessoryType::MarkerControlBoard => Self::MarkerControlBoard,
        }
    }
}
impl From<AccessoryType> for common::accessory::AccessoryType {
    fn from(v: AccessoryType) -> Self {
        match v {
            AccessoryType::DryFireMag => Self::DryFireMag,
            AccessoryType::MantisX => Self::MantisX,
            AccessoryType::MarkerControlBoard => Self::MarkerControlBoard,
        }
    }
}

impl From<common::accessory::AccessoryInfo> for AccessoryInfo {
    fn from(v: common::accessory::AccessoryInfo) -> Self {
        Self {
            name: v.name,
            ty: AccessoryType::from(v.ty) as i32,
            assignment: v.assignment.map(NonZero::get).unwrap_or(0),
            screen_id: v.screen_id.map(u32::from),
            radio: Some(v.radio.into()),
            features: v
                .features
                .into_iter()
                .map(|f| AccessoryFeature::from(f) as i32)
                .collect(),
        }
    }
}
impl TryFrom<AccessoryInfo> for common::accessory::AccessoryInfo {
    type Error = &'static str;

    fn try_from(v: AccessoryInfo) -> Result<Self, Self::Error> {
        Ok(Self {
            name: v.name,
            ty: AccessoryType::try_from(v.ty)
                .map_err(|_conversion_error| "unknown accessory type")?
                .into(),
            assignment: NonZero::new(v.assignment),
            radio: v
                .radio
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or_default(),
            screen_id: v
                .screen_id
                .map(|id| {
                    u8::try_from(id)
                        .ok()
                        .filter(|id| *id <= odyssey_common::MAX_SCREEN_ID)
                        .ok_or("screen id out of range")
                })
                .transpose()?,
            features: v
                .features
                .into_iter()
                .map(|f| {
                    AccessoryFeature::try_from(f)
                        .map(common::accessory::AccessoryFeature::from)
                        .map_err(|_conversion_error| "unknown accessory feature")
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<common::accessory::AccessoryMap> for AccessoryMapReply {
    fn from(map: common::accessory::AccessoryMap) -> Self {
        let mut accessory_map = std::collections::HashMap::new();
        for (address, (info, connected)) in map {
            accessory_map.insert(
                address_to_key(address),
                AccessoryStatus {
                    accessory: Some(info.into()),
                    connected,
                },
            );
        }
        Self { accessory_map }
    }
}
impl TryFrom<AccessoryMapReply> for common::accessory::AccessoryMap {
    type Error = &'static str;

    fn try_from(v: AccessoryMapReply) -> Result<Self, Self::Error> {
        v.accessory_map
            .into_iter()
            .map(|(key, status)| {
                let info = status
                    .accessory
                    .ok_or("accessory status missing accessory")?;
                Ok((address_from_key(key)?, (info.try_into()?, status.connected)))
            })
            .collect()
    }
}

impl TryFrom<AccessoryInfoMap> for common::accessory::AccessoryInfoMap {
    type Error = &'static str;

    fn try_from(v: AccessoryInfoMap) -> Result<Self, Self::Error> {
        v.accessory_info_map
            .into_iter()
            .map(|(key, info)| Ok((address_from_key(key)?, info.try_into()?)))
            .collect()
    }
}
impl From<common::accessory::AccessoryInfoMap> for AccessoryInfoMap {
    fn from(map: common::accessory::AccessoryInfoMap) -> Self {
        let mut accessory_info_map = std::collections::HashMap::new();
        for (address, info) in map {
            accessory_info_map.insert(address_to_key(address), info.into());
        }
        Self { accessory_info_map }
    }
}

impl From<common::accessory::AccessoryRadio> for crate::AccessoryRadio {
    fn from(radio: common::accessory::AccessoryRadio) -> Self {
        use common::accessory::AccessoryRadio as Radio;
        let (kind, id) = match radio {
            Radio::System => (crate::AccessoryRadioKind::System, String::new()),
            Radio::Native { id } => (crate::AccessoryRadioKind::Native, id),
            Radio::Dongle { id } => (crate::AccessoryRadioKind::Dongle, id),
        };
        Self {
            kind: kind as i32,
            id,
        }
    }
}

impl TryFrom<crate::AccessoryRadio> for common::accessory::AccessoryRadio {
    type Error = &'static str;

    fn try_from(value: crate::AccessoryRadio) -> Result<Self, Self::Error> {
        use crate::AccessoryRadioKind;
        let kind =
            AccessoryRadioKind::try_from(value.kind).map_err(|_| "unknown accessory radio kind")?;
        match kind {
            AccessoryRadioKind::System if value.id.is_empty() => Ok(Self::System),
            AccessoryRadioKind::Native if valid_radio_id(&value.id) => {
                Ok(Self::Native { id: value.id })
            }
            AccessoryRadioKind::Dongle if valid_radio_id(&value.id) => {
                Ok(Self::Dongle { id: value.id })
            }
            _ => Err("invalid accessory radio id"),
        }
    }
}

fn valid_radio_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control) && id.trim() == id
}

impl From<common::accessory::AccessoryConnectionState> for crate::AccessoryConnectionState {
    fn from(value: common::accessory::AccessoryConnectionState) -> Self {
        use common::accessory::AccessoryConnectionState as State;
        match value {
            State::Idle => Self::Idle,
            State::Connecting => Self::Connecting,
            State::Connected => Self::Connected,
            State::Failed => Self::Failed,
            State::RadioUnavailable => Self::RadioUnavailable,
        }
    }
}

impl From<crate::AccessoryConnectionState> for common::accessory::AccessoryConnectionState {
    fn from(value: crate::AccessoryConnectionState) -> Self {
        match value {
            crate::AccessoryConnectionState::Idle => Self::Idle,
            crate::AccessoryConnectionState::Connecting => Self::Connecting,
            crate::AccessoryConnectionState::Connected => Self::Connected,
            crate::AccessoryConnectionState::Failed => Self::Failed,
            crate::AccessoryConnectionState::RadioUnavailable => Self::RadioUnavailable,
        }
    }
}

impl From<common::accessory::AccessoryRadioInfo> for crate::AccessoryRadioInfo {
    fn from(value: common::accessory::AccessoryRadioInfo) -> Self {
        Self {
            radio: Some(value.radio.into()),
            name: value.name,
            is_default: value.is_default,
            available: value.available,
            accessory_supported: value.accessory_supported,
            connected_accessories: value.connected_accessories,
            connecting_accessories: value.connecting_accessories,
            connected_trackers: value.connected_trackers,
            capacity: value.capacity,
            accessory_capacity: value.accessory_capacity,
            connections: value
                .connections
                .into_iter()
                .map(|connection| crate::AccessoryConnectionInfo {
                    address: address_to_key(connection.address),
                    state: crate::AccessoryConnectionState::from(connection.state) as i32,
                    error: connection.error,
                })
                .collect(),
            nearby: value
                .nearby
                .into_iter()
                .map(|nearby| crate::DiscoveredAccessory {
                    address: address_to_key(nearby.address),
                    name: nearby.name,
                    ty: AccessoryType::from(nearby.ty) as i32,
                    pairing_window_expires_at_ms: nearby.pairing_window_expires_at_ms,
                })
                .collect(),
            error: value.error,
        }
    }
}

impl TryFrom<crate::AccessoryRadioInfo> for common::accessory::AccessoryRadioInfo {
    type Error = &'static str;

    fn try_from(value: crate::AccessoryRadioInfo) -> Result<Self, Self::Error> {
        let links = value
            .connected_accessories
            .checked_add(value.connected_trackers)
            .and_then(|n| n.checked_add(value.connecting_accessories))
            .ok_or("radio count overflow")?;
        if value.capacity.is_some_and(|cap| links > cap)
            || value.accessory_capacity.is_some_and(|cap| {
                value
                    .connected_accessories
                    .saturating_add(value.connecting_accessories)
                    > cap
            })
            || matches!((value.capacity, value.accessory_capacity), (Some(total), Some(accessories)) if accessories > total)
        {
            return Err("radio count exceeds reported capacity");
        }
        Ok(Self {
            radio: value.radio.ok_or("radio info missing radio")?.try_into()?,
            name: value.name,
            is_default: value.is_default,
            available: value.available,
            accessory_supported: value.accessory_supported,
            connected_accessories: value.connected_accessories,
            connecting_accessories: value.connecting_accessories,
            connected_trackers: value.connected_trackers,
            capacity: value.capacity,
            accessory_capacity: value.accessory_capacity,
            connections: value
                .connections
                .into_iter()
                .map(|connection| {
                    Ok(common::accessory::AccessoryConnectionInfo {
                        address: address_from_key(connection.address)?,
                        state: crate::AccessoryConnectionState::try_from(connection.state)
                            .map_err(|_| "unknown accessory connection state")?
                            .into(),
                        error: connection.error,
                    })
                })
                .collect::<Result<_, Self::Error>>()?,
            nearby: value
                .nearby
                .into_iter()
                .map(|nearby| {
                    Ok(common::accessory::DiscoveredAccessory {
                        address: address_from_key(nearby.address)?,
                        name: nearby.name,
                        ty: AccessoryType::try_from(nearby.ty)
                            .map_err(|_| "unknown accessory type")?
                            .into(),
                        pairing_window_expires_at_ms: nearby.pairing_window_expires_at_ms,
                    })
                })
                .collect::<Result<_, Self::Error>>()?,
            error: value.error,
        })
    }
}

impl From<Vec<common::accessory::AccessoryRadioInfo>> for crate::AccessoryRadiosReply {
    fn from(radios: Vec<common::accessory::AccessoryRadioInfo>) -> Self {
        Self {
            radios: radios.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<crate::AccessoryRadiosReply> for Vec<common::accessory::AccessoryRadioInfo> {
    type Error = &'static str;

    fn try_from(value: crate::AccessoryRadiosReply) -> Result<Self, Self::Error> {
        value.radios.into_iter().map(TryInto::try_into).collect()
    }
}

impl TryFrom<crate::EnrollAccessoryRequest> for ([u8; 6], common::accessory::AccessoryInfo) {
    type Error = &'static str;

    fn try_from(value: crate::EnrollAccessoryRequest) -> Result<Self, Self::Error> {
        Ok((
            address_from_key(value.address)?,
            value
                .info
                .ok_or("enrollment missing accessory info")?
                .try_into()?,
        ))
    }
}

impl TryFrom<crate::ResetMarkerModeRequest> for [u8; 6] {
    type Error = &'static str;

    fn try_from(value: crate::ResetMarkerModeRequest) -> Result<Self, Self::Error> {
        address_from_key(value.address)
    }
}

#[cfg(test)]
mod tests {
    use prost::Message as _;

    use super::*;

    #[test]
    fn pairing_window_wire_presence_distinguishes_unknown_closed_and_open() {
        for (bytes, expiry) in [
            (&[8, 1][..], None),
            (&[8, 1, 32, 0][..], Some(0)),
            (&[8, 1, 32, 0xe8, 0x07][..], Some(1000)),
        ] {
            let accessory = crate::DiscoveredAccessory::decode(bytes).unwrap();
            assert_eq!(accessory.pairing_window_expires_at_ms, expiry);
            assert_eq!(accessory.encode_to_vec(), bytes);
        }
    }

    #[test]
    fn radio_routes_reject_unknown_kinds_and_malformed_fixed_ids() {
        for radio in [
            crate::AccessoryRadio {
                kind: 99,
                id: String::new(),
            },
            crate::AccessoryRadio {
                kind: crate::AccessoryRadioKind::System as i32,
                id: "unexpected".into(),
            },
            crate::AccessoryRadio {
                kind: crate::AccessoryRadioKind::Native as i32,
                id: String::new(),
            },
            crate::AccessoryRadio {
                kind: crate::AccessoryRadioKind::Dongle as i32,
                id: "usb\nid".into(),
            },
        ] {
            assert!(common::accessory::AccessoryRadio::try_from(radio).is_err());
        }
        let legacy = common::accessory::AccessoryInfo::try_from(AccessoryInfo::default()).unwrap();
        assert_eq!(legacy.radio, common::accessory::AccessoryRadio::System);
    }

    #[test]
    fn unknown_capacity_accepts_eighth_link_and_keeps_original_errors() {
        let native = crate::AccessoryRadioInfo {
            radio: Some(
                common::accessory::AccessoryRadio::Native {
                    id: "00:11:22:33:44:55".into(),
                }
                .into(),
            ),
            connected_accessories: 8,
            connecting_accessories: 2,
            available: true,
            accessory_supported: true,
            error: Some("org.bluez.Error.Failed: connection canceled".into()),
            ..Default::default()
        };
        let decoded = common::accessory::AccessoryRadioInfo::try_from(native.clone()).unwrap();
        assert_eq!(decoded.connected_accessories, 8);
        assert_eq!(decoded.capacity, None);
        assert_eq!(
            decoded.error.as_deref(),
            Some("org.bluez.Error.Failed: connection canceled")
        );
        let mut bounded = native;
        bounded.capacity = Some(9);
        assert_eq!(
            common::accessory::AccessoryRadioInfo::try_from(bounded).unwrap_err(),
            "radio count exceeds reported capacity"
        );
    }

    #[test]
    fn unknown_accessory_enums_reject_the_update() {
        let map = AccessoryInfoMap {
            accessory_info_map: [(
                1,
                AccessoryInfo {
                    ty: i32::MAX,
                    ..AccessoryInfo::default()
                },
            )]
            .into(),
        };
        assert!(common::accessory::AccessoryInfoMap::try_from(map).is_err());
        assert!(
            common::accessory::AccessoryInfo::try_from(AccessoryInfo {
                features: vec![i32::MAX],
                ..AccessoryInfo::default()
            })
            .is_err()
        );
    }

    #[test]
    fn marker_screen_assignment_round_trips_and_rejects_out_of_range_screens() {
        let mut board = common::accessory::AccessoryInfo::with_defaults(
            "markers",
            common::accessory::AccessoryType::MarkerControlBoard,
        );
        for screen in [None, Some(0), Some(odyssey_common::MAX_SCREEN_ID)] {
            board.screen_id = screen;
            let encoded = AccessoryInfo::from(board.clone());
            assert_eq!(
                common::accessory::AccessoryInfo::try_from(encoded).unwrap(),
                board
            );
        }
        for screen in [u32::from(odyssey_common::MAX_SCREEN_ID) + 1, 256, u32::MAX] {
            let mut encoded = AccessoryInfo::from(board.clone());
            encoded.screen_id = Some(screen);
            assert_eq!(
                common::accessory::AccessoryInfo::try_from(encoded).unwrap_err(),
                "screen id out of range",
            );
        }
    }

    #[test]
    fn accessory_status_requires_its_info() {
        let reply = AccessoryMapReply {
            accessory_map: [(
                1,
                AccessoryStatus {
                    accessory: None,
                    connected: true,
                },
            )]
            .into(),
        };
        assert!(common::accessory::AccessoryMap::try_from(reply).is_err());
    }
}
