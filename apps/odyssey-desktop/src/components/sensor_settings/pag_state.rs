use dioxus::prelude::ReadableExt as _;
use odyssey_hub_common::device::Device;

use super::field_issue::{FieldIssue, parse_bounded};
use super::registers::{read_u16_reg, write_u16_reg};
use crate::hub::HubContext;

#[derive(Clone, PartialEq, Default)]
pub(super) struct PagState {
    pub(super) fps: String,
    pub(super) exposure_us: String,
    pub(super) gain: String,
    pub(super) light_threshold: String,
    pub(super) area_lower: String,
    pub(super) area_upper: String,
}

// PAG7665 register addresses (used for product_id 0x5211)
pub(super) async fn pag_load_from_device(
    hub: &HubContext,
    device: &Device,
) -> anyhow::Result<PagState> {
    let port: u8 = 0; // PAG always uses NF port
    let fps = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, 0x00, 0x13)
        .await?;
    let expo_raw = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, 0x00, 0x67)
        .await?;
    let gain = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, 0x00, 0x68)
        .await?;
    let light_threshold = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, 0x00, 0x6D)
        .await?;
    let area_lower = read_u16_reg(hub, device, port, 0x00, [0x6E, 0x6F]).await?;
    let area_upper = read_u16_reg(hub, device, port, 0x00, [0x70, 0x71]).await?;

    // Exposure encoding: value in units of 100us, bit 7 = led_always_on
    let exposure_us = u16::from(expo_raw & 0x7F) * 100;

    Ok(PagState {
        fps: fps.to_string(),
        exposure_us: exposure_us.to_string(),
        gain: gain.to_string(),
        light_threshold: light_threshold.to_string(),
        area_lower: area_lower.to_string(),
        area_upper: area_upper.to_string(),
    })
}

/// PAG sensor settings after validation.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct PagValues {
    pub(super) fps: u8,
    pub(super) exposure_us: u16,
    pub(super) gain: u8,
    pub(super) light_threshold: u8,
    pub(super) area_lower: u16,
    pub(super) area_upper: u16,
}

// The exposure register holds 100us units in its low 7 bits, so 12700us is the
// ceiling and anything finer than 100us is truncated by the device.
pub(super) const PAG_MAX_EXPOSURE_US: u32 = 12_700;

pub(super) fn pag_validate(state: &PagState) -> Result<PagValues, Vec<FieldIssue>> {
    let mut issues = Vec::new();
    let mut take = |field, raw: &str, max| match parse_bounded(field, raw, max) {
        Ok(v) => Some(v),
        Err(e) => {
            issues.push(e);
            None
        }
    };

    const U8: u32 = u8::MAX as u32;
    const U16: u32 = u16::MAX as u32;
    let fps = take(PAG_FPS, &state.fps, U8);
    let exposure = take(PAG_EXPOSURE, &state.exposure_us, PAG_MAX_EXPOSURE_US);
    let gain = take(PAG_GAIN, &state.gain, U8);
    let light = take(PAG_LIGHT, &state.light_threshold, U8);
    let area_lower = take(PAG_AREA_MIN, &state.area_lower, U16);
    let area_upper = take(PAG_AREA_MAX, &state.area_upper, U16);

    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(PagValues {
        fps: fps.unwrap() as u8,
        exposure_us: exposure.unwrap() as u16,
        gain: gain.unwrap() as u8,
        light_threshold: light.unwrap() as u8,
        area_lower: area_lower.unwrap() as u16,
        area_upper: area_upper.unwrap() as u16,
    })
}

pub(super) async fn pag_apply_to_device(
    hub: &HubContext,
    device: &Device,
    values: &PagValues,
) -> anyhow::Result<()> {
    let port: u8 = 0;
    let led_always_on = true;
    let expo_reg = (values.exposure_us / 100) as u8 | (u8::from(led_always_on) << 7);

    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x00, 0x13, values.fps)
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x00, 0x67, expo_reg)
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x00, 0x68, values.gain)
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x00, 0x6D, values.light_threshold)
        .await?;
    write_u16_reg(hub, device, port, 0x00, [0x6E, 0x6F], values.area_lower).await?;
    write_u16_reg(hub, device, port, 0x00, [0x70, 0x71], values.area_upper).await?;
    Ok(())
}

pub(super) fn pag_load_defaults() -> PagState {
    PagState {
        fps: "180".into(),
        exposure_us: "2000".into(),
        gain: "2".into(),
        light_threshold: "120".into(),
        area_lower: "10".into(),
        area_upper: "65535".into(),
    }
}

pub(super) const PAG_FPS: &str = "Frame rate";
pub(super) const PAG_EXPOSURE: &str = "Exposure";
pub(super) const PAG_GAIN: &str = "Gain";
pub(super) const PAG_LIGHT: &str = "Light threshold";
pub(super) const PAG_AREA_MIN: &str = "Area threshold min";
pub(super) const PAG_AREA_MAX: &str = "Area threshold max";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::sensor_settings::field_issue::test_support::{issues, ok};

    #[test]
    fn pag_exposure_encoding() {
        let v = ok(pag_validate(&pag_load_defaults()));
        assert_eq!(v.exposure_us, 2000);
        assert_eq!((v.exposure_us / 100) as u8 | 0x80, 0x94);

        let mut bad = pag_load_defaults();
        bad.exposure_us = "13000".into();
        assert_eq!(issues(pag_validate(&bad))[0].field, PAG_EXPOSURE);
    }
}
