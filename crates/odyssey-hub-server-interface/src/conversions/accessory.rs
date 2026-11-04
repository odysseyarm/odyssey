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
            ty: AccessoryType::try_from(v.ty)
                .expect("accessory type is a defined AccessoryType value")
                .into(),
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
                        AccessoryFeature::try_from(f)
                            .expect("accessory feature is a defined AccessoryFeature value"),
                    )
                })
                .collect(),
        }
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
impl From<AccessoryMapReply> for common::accessory::AccessoryMap {
    fn from(v: AccessoryMapReply) -> Self {
        v.accessory_map
            .into_iter()
            .map(|(key, status)| {
                let info = status
                    .accessory
                    .expect("accessory status carries its accessory");
                (address_from_key(key), (info.into(), status.connected))
            })
            .collect()
    }
}

impl From<AccessoryInfoMap> for common::accessory::AccessoryInfoMap {
    fn from(v: AccessoryInfoMap) -> Self {
        v.accessory_info_map
            .into_iter()
            .map(|(key, info)| (address_from_key(key), info.into()))
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
