use std::collections::HashSet;
use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AccessoryType {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    DryFireMag,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    MantisX,
    /// Since: Odyssey Desktop 0.7.4.
    MarkerControlBoard,
}

impl AccessoryType {
    /// Since: Odyssey Desktop 0.7.4.
    pub const ALL: [Self; 3] = [Self::MarkerControlBoard, Self::DryFireMag, Self::MantisX];

    /// Since: Odyssey Desktop 0.7.4.
    pub const fn name(self) -> &'static str {
        match self {
            Self::DryFireMag => "DryFireMag",
            Self::MantisX => "MantisX",
            Self::MarkerControlBoard => "Marker Control Board",
        }
    }

    /// Since: Odyssey Desktop 0.7.4.
    pub const fn is_first_party(self) -> bool {
        matches!(self, Self::MarkerControlBoard)
    }
}

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AccessoryFeature {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    Impact,
}

/// Since: Odyssey Desktop 0.7.4.
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccessoryRadio {
    /// Since: Odyssey Desktop 0.7.4.
    #[default]
    System,
    /// Since: Odyssey Desktop 0.7.4.
    Native {
        /// Since: Odyssey Desktop 0.7.4.
        id: String,
    },
    /// Since: Odyssey Desktop 0.7.4.
    Dongle {
        /// Since: Odyssey Desktop 0.7.4.
        id: String,
    },
}

/// Since: Odyssey Desktop 0.7.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessoryConnectionState {
    /// Since: Odyssey Desktop 0.7.4.
    Idle,
    /// Since: Odyssey Desktop 0.7.4.
    Connecting,
    /// Since: Odyssey Desktop 0.7.4.
    Connected,
    /// Since: Odyssey Desktop 0.7.4.
    Failed,
    /// Since: Odyssey Desktop 0.7.4.
    RadioUnavailable,
}

/// Since: Odyssey Desktop 0.7.4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessoryConnectionInfo {
    /// Since: Odyssey Desktop 0.7.4.
    pub address: [u8; 6],
    /// Since: Odyssey Desktop 0.7.4.
    pub state: AccessoryConnectionState,
    /// Since: Odyssey Desktop 0.7.4.
    pub error: Option<String>,
}

/// Since: Odyssey Desktop 0.7.4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredAccessory {
    /// Since: Odyssey Desktop 0.7.4.
    pub address: [u8; 6],
    /// Since: Odyssey Desktop 0.7.4.
    pub name: String,
    /// Since: Odyssey Desktop 0.7.4.
    pub ty: AccessoryType,
    /// `None` is unknown, `Some(0)` is closed, anything else is Unix-epoch milliseconds.
    ///
    /// Since: Odyssey Desktop 0.7.4.
    #[serde(default)]
    pub pairing_window_expires_at_ms: Option<u64>,
}

/// Since: Odyssey Desktop 0.7.4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessoryRadioInfo {
    /// Since: Odyssey Desktop 0.7.4.
    pub radio: AccessoryRadio,
    /// Since: Odyssey Desktop 0.7.4.
    pub name: String,
    /// Since: Odyssey Desktop 0.7.4.
    pub is_default: bool,
    /// Since: Odyssey Desktop 0.7.4.
    pub available: bool,
    /// Since: Odyssey Desktop 0.7.4.
    pub accessory_supported: bool,
    /// Since: Odyssey Desktop 0.7.4.
    pub connected_accessories: u32,
    /// Since: Odyssey Desktop 0.7.4.
    pub connecting_accessories: u32,
    /// Since: Odyssey Desktop 0.7.4.
    pub connected_trackers: u32,
    /// Since: Odyssey Desktop 0.7.4.
    pub capacity: Option<u32>,
    /// Since: Odyssey Desktop 0.7.4.
    pub accessory_capacity: Option<u32>,
    /// Since: Odyssey Desktop 0.7.4.
    pub connections: Vec<AccessoryConnectionInfo>,
    /// Since: Odyssey Desktop 0.7.4.
    pub nearby: Vec<DiscoveredAccessory>,
    /// Since: Odyssey Desktop 0.7.4.
    pub error: Option<String>,
}

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessoryInfo {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub name: String,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub ty: AccessoryType,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub assignment: Option<NonZeroU64>,
    /// Since: Odyssey Desktop 0.7.4.
    #[serde(default)]
    pub screen_id: Option<u8>,
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    #[serde(default)]
    pub features: HashSet<AccessoryFeature>,
    /// Since: Odyssey Desktop 0.7.4.
    #[serde(default)]
    pub radio: AccessoryRadio,
}

impl AccessoryInfo {
    /// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
    pub fn with_defaults(name: impl Into<String>, ty: AccessoryType) -> Self {
        let mut features = HashSet::new();
        match ty {
            AccessoryType::DryFireMag | AccessoryType::MantisX => {
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

/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub type AccessoryConnected = bool;
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub type AccessoryMap = std::collections::HashMap<[u8; 6], (AccessoryInfo, AccessoryConnected)>;
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
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
