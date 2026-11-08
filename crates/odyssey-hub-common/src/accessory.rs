use std::collections::HashSet;
use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AccessoryType {
    DryFireMag,
    MantisX,
    MarkerControlBoard,
}

impl AccessoryType {
    pub const ALL: [Self; 3] = [Self::MarkerControlBoard, Self::DryFireMag, Self::MantisX];

    pub const fn name(self) -> &'static str {
        match self {
            Self::DryFireMag => "DryFireMag",
            Self::MantisX => "MantisX",
            Self::MarkerControlBoard => "Marker Control Board",
        }
    }

    pub const fn is_first_party(self) -> bool {
        matches!(self, Self::MarkerControlBoard)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AccessoryFeature {
    Impact,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccessoryRadio {
    #[default]
    System,
    Native {
        id: String,
    },
    Dongle {
        id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessoryConnectionState {
    Idle,
    Connecting,
    Connected,
    Failed,
    RadioUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessoryConnectionInfo {
    pub address: [u8; 6],
    pub state: AccessoryConnectionState,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredAccessory {
    pub address: [u8; 6],
    pub name: String,
    pub ty: AccessoryType,
    /// None: unknown; Some(0): closed; otherwise Unix-epoch milliseconds.
    #[serde(default)]
    pub pairing_window_expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessoryRadioInfo {
    pub radio: AccessoryRadio,
    pub name: String,
    pub is_default: bool,
    pub available: bool,
    pub accessory_supported: bool,
    pub connected_accessories: u32,
    pub connecting_accessories: u32,
    pub connected_trackers: u32,
    pub capacity: Option<u32>,
    pub accessory_capacity: Option<u32>,
    pub connections: Vec<AccessoryConnectionInfo>,
    pub nearby: Vec<DiscoveredAccessory>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessoryInfo {
    pub name: String,
    pub ty: AccessoryType,
    pub assignment: Option<NonZeroU64>,
    #[serde(default)]
    pub screen_id: Option<u8>,
    #[serde(default)]
    pub features: HashSet<AccessoryFeature>,
    #[serde(default)]
    pub radio: AccessoryRadio,
}

impl AccessoryInfo {
    pub fn with_defaults(name: impl Into<String>, ty: AccessoryType) -> Self {
        let mut features = HashSet::new();
        match ty {
            AccessoryType::DryFireMag => {
                features.insert(AccessoryFeature::Impact);
            }
            AccessoryType::MantisX => {
                features.insert(AccessoryFeature::Impact);
            }
            AccessoryType::MarkerControlBoard => {}
        }
        Self {
            name: name.into(),
            ty,
            assignment: None,
            screen_id: None,
            features,
            radio: AccessoryRadio::System,
        }
    }
}

pub type AccessoryConnected = bool;
pub type AccessoryMap = std::collections::HashMap<[u8; 6], (AccessoryInfo, AccessoryConnected)>;
pub type AccessoryInfoMap = std::collections::HashMap<[u8; 6], AccessoryInfo>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_screen_six_config_keeps_assignment_and_defaults_to_system_radio() {
        let info: AccessoryInfo = serde_json::from_str(
            r#"{"name":"markers","ty":"MarkerControlBoard","assignment":null,"screen_id":5}"#,
        )
        .unwrap();
        assert_eq!(info.radio, AccessoryRadio::System);
        assert_eq!(info.screen_id, Some(odyssey_common::MAX_SCREEN_ID));
        assert!(info.features.is_empty());
        let mut selected = info;
        selected.radio = AccessoryRadio::Native {
            id: "00:11:22:33:44:55".into(),
        };
        let reloaded: AccessoryInfo =
            serde_json::from_str(&serde_json::to_string(&selected).unwrap()).unwrap();
        assert_eq!(reloaded, selected);
    }

    #[test]
    fn marker_board_is_first_party_without_impact_or_assignment() {
        let marker = AccessoryInfo::with_defaults(
            AccessoryType::MarkerControlBoard.name(),
            AccessoryType::MarkerControlBoard,
        );
        assert!(marker.ty.is_first_party());
        assert!(marker.features.is_empty());
        assert_eq!(marker.assignment, None);
        for ty in [AccessoryType::DryFireMag, AccessoryType::MantisX] {
            assert!(!ty.is_first_party());
            assert!(
                AccessoryInfo::with_defaults(ty.name(), ty)
                    .features
                    .contains(&AccessoryFeature::Impact)
            );
        }
    }
}
