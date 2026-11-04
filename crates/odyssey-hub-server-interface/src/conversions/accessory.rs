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
impl TryFrom<AccessoryInfo> for common::accessory::AccessoryInfo {
    type Error = &'static str;

    fn try_from(v: AccessoryInfo) -> Result<Self, Self::Error> {
        Ok(Self {
            name: v.name,
            ty: AccessoryType::try_from(v.ty)
                .map_err(|_conversion_error| "unknown accessory type")?
                .into(),
            assignment: NonZero::new(v.assignment),
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

#[cfg(test)]
mod tests {
    use super::*;

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
