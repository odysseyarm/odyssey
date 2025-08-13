use serde::{Deserialize, Serialize};
use std::{collections::HashSet, num::NonZeroU64};

#[repr(C)]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AccessoryType {
    DryFireMag,
    BlackbeardX,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AccessoryFeature {
    Impact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessoryInfo {
    pub name: String,
    pub ty: AccessoryType,
    pub assignment: Option<NonZeroU64>,
    #[serde(default)]
    pub features: HashSet<AccessoryFeature>,
}

impl AccessoryInfo {
    pub fn with_defaults(name: impl Into<String>, ty: AccessoryType) -> Self {
        let mut features = HashSet::new();
        match ty {
            AccessoryType::DryFireMag => {
                features.insert(AccessoryFeature::Impact);
            }
            AccessoryType::BlackbeardX => {
                features.insert(AccessoryFeature::Impact);
            }
        }
        Self {
            name: name.into(),
            ty,
            assignment: None,
            features,
        }
    }
}

pub type AccessoryConnected = bool;
pub type AccessoryMap = std::collections::HashMap<[u8; 6], (AccessoryInfo, AccessoryConnected)>;
pub type AccessoryInfoMap = std::collections::HashMap<[u8; 6], AccessoryInfo>;
