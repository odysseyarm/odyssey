use std::num::NonZero;

use odyssey_hub_common as common;

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
