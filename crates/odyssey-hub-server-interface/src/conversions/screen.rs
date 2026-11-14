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
impl TryFrom<ScreenInfoReply> for common::ScreenInfo {
    type Error = &'static str;

    fn try_from(v: ScreenInfoReply) -> Result<Self, Self::Error> {
        let bounds = v.bounds.ok_or("screen info missing bounds")?;
        Ok(Self {
            id: u8::try_from(v.id)
                .ok()
                .filter(|id| *id <= odyssey_common::MAX_SCREEN_ID)
                .ok_or("screen id out of range")?,
            bounds: [
                bounds.tl.ok_or("screen bounds missing top-left")?.into(),
                bounds.tr.ok_or("screen bounds missing top-right")?.into(),
                bounds.bl.ok_or("screen bounds missing bottom-left")?.into(),
                bounds
                    .br
                    .ok_or("screen bounds missing bottom-right")?
                    .into(),
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_screen_bounds_are_errors() {
        common::ScreenInfo::try_from(ScreenInfoReply::default())
            .expect_err("missing bounds must be rejected");
        common::ScreenInfo::try_from(ScreenInfoReply {
            id: 1,
            bounds: Some(ScreenBounds {
                tl: Some(Vector2::default()),
                tr: Some(Vector2::default()),
                bl: Some(Vector2::default()),
                br: None,
            }),
        })
        .expect_err("incomplete bounds must be rejected");
    }
}
