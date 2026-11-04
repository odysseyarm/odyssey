use odyssey_hub_common as common;

use crate::{ScreenBounds, ScreenInfoReply, Vector2};

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
