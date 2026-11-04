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
        let bounds = v.bounds.expect("screen info carries its bounds");
        Self {
            id: v.id as u8,
            bounds: [
                bounds.tl.expect("bounds carry top-left").into(),
                bounds.tr.expect("bounds carry top-right").into(),
                bounds.bl.expect("bounds carry bottom-left").into(),
                bounds.br.expect("bounds carry bottom-right").into(),
            ],
        }
    }
}
