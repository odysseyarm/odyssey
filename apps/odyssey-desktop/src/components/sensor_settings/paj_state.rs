use dioxus::prelude::ReadableExt as _;
use odyssey_hub_common::device::Device;

use super::field_issue::{FieldIssue, parse_bounded};
use super::gain_table::{GAIN_TABLE, gain_index_from_reg};
use super::registers::{read_u16_reg, read_u32_3byte, write_u16_reg, write_u32_3byte};
use crate::hub::HubContext;

#[derive(Clone, PartialEq)]
pub(super) struct PajState {
    pub(super) brightness_threshold: String,
    pub(super) noise_threshold: String,
    pub(super) area_threshold_min: String,
    pub(super) area_threshold_max: String,
    pub(super) max_object_cnt: String,
    pub(super) operation_mode: String,
    pub(super) exposure_time: String,
    pub(super) frame_period: String,
    pub(super) frame_subtraction: String,
    pub(super) gain_index: String,
    pub(super) resolution_x: String,
    pub(super) resolution_y: String,
}

impl Default for PajState {
    fn default() -> Self {
        Self {
            brightness_threshold: String::new(),
            noise_threshold: String::new(),
            area_threshold_min: String::new(),
            area_threshold_max: String::new(),
            max_object_cnt: String::new(),
            operation_mode: "0".into(),
            exposure_time: String::new(),
            frame_period: String::new(),
            frame_subtraction: "0".into(),
            gain_index: "0".into(),
            resolution_x: String::new(),
            resolution_y: String::new(),
        }
    }
}

pub(super) async fn paj_load_from_device(
    hub: &HubContext,
    device: &Device,
    port: u8,
) -> anyhow::Result<PajState> {
    let brightness_threshold = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, 0x0C, 0x47)
        .await?;
    let noise_threshold = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, 0x00, 0x0F)
        .await?;
    let area_threshold_min = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, 0x0C, 0x46)
        .await?;
    let area_threshold_max = read_u16_reg(hub, device, port, 0x00, [0x0B, 0x0C]).await?;
    let max_object_cnt = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, 0x00, 0x19)
        .await?;
    let operation_mode = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, 0x00, 0x12)
        .await?;
    let exposure_time = read_u16_reg(hub, device, port, 0x01, [0x0E, 0x0F]).await?;
    let frame_period = read_u32_3byte(hub, device, port, 0x0C, [0x07, 0x08, 0x09]).await?;
    let frame_subtraction = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, 0x00, 0x28)
        .await?;
    let gain_1 = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, 0x01, 0x05)
        .await?;
    let gain_2 = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, 0x01, 0x06)
        .await?;
    let resolution_x = read_u16_reg(hub, device, port, 0x0C, [0x60, 0x61]).await?;
    let resolution_y = read_u16_reg(hub, device, port, 0x0C, [0x62, 0x63]).await?;

    Ok(PajState {
        brightness_threshold: brightness_threshold.to_string(),
        noise_threshold: noise_threshold.to_string(),
        area_threshold_min: area_threshold_min.to_string(),
        area_threshold_max: area_threshold_max.to_string(),
        max_object_cnt: max_object_cnt.to_string(),
        operation_mode: operation_mode.to_string(),
        exposure_time: exposure_time.to_string(),
        frame_period: frame_period.to_string(),
        frame_subtraction: frame_subtraction.to_string(),
        gain_index: gain_index_from_reg(gain_1, gain_2).to_string(),
        resolution_x: resolution_x.to_string(),
        resolution_y: resolution_y.to_string(),
    })
}

/// PAJ sensor settings after validation, in the units the registers expect.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct PajValues {
    pub(super) brightness_threshold: u8,
    pub(super) noise_threshold: u8,
    pub(super) area_threshold_min: u8,
    pub(super) area_threshold_max: u16,
    pub(super) max_object_cnt: u8,
    pub(super) operation_mode: u8,
    pub(super) exposure_time: u16,
    pub(super) frame_period: u32,
    pub(super) frame_subtraction: u8,
    pub(super) gain_index: usize,
    pub(super) resolution_x: u16,
    pub(super) resolution_y: u16,
}

pub(super) fn paj_validate(state: &PajState) -> Result<PajValues, Vec<FieldIssue>> {
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
    let brightness = take(PAJ_BRIGHTNESS, &state.brightness_threshold, U8);
    let noise = take(PAJ_NOISE, &state.noise_threshold, U8);
    let area_min = take(PAJ_AREA_MIN, &state.area_threshold_min, U8);
    let area_max = take(PAJ_AREA_MAX, &state.area_threshold_max, U16);
    let max_objects = take(PAJ_MAX_OBJECTS, &state.max_object_cnt, 16);
    let exposure = take(PAJ_EXPOSURE, &state.exposure_time, U16);
    // 24-bit register, written as three bytes.
    let frame_period = take(PAJ_FRAME_PERIOD, &state.frame_period, 0xFF_FFFF);
    let resolution_x = take(PAJ_RESOLUTION_X, &state.resolution_x, 4095);
    let resolution_y = take(PAJ_RESOLUTION_Y, &state.resolution_y, 4095);

    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(PajValues {
        brightness_threshold: brightness.unwrap() as u8,
        noise_threshold: noise.unwrap() as u8,
        area_threshold_min: area_min.unwrap() as u8,
        area_threshold_max: area_max.unwrap() as u16,
        max_object_cnt: max_objects.unwrap() as u8,
        // Dropdowns, so these cannot carry user typos.
        operation_mode: state.operation_mode.parse().unwrap_or(0),
        exposure_time: exposure.unwrap() as u16,
        frame_period: frame_period.unwrap(),
        frame_subtraction: state.frame_subtraction.parse().unwrap_or(0),
        gain_index: state
            .gain_index
            .parse::<usize>()
            .unwrap_or(0)
            .min(GAIN_TABLE.len() - 1),
        resolution_x: resolution_x.unwrap() as u16,
        resolution_y: resolution_y.unwrap() as u16,
    })
}

pub(super) async fn paj_apply_to_device(
    hub: &HubContext,
    device: &Device,
    port: u8,
    values: &PajValues,
) -> anyhow::Result<()> {
    let gain = &GAIN_TABLE[values.gain_index];

    hub.client
        .peek()
        .clone()
        .write_register(
            device.clone(),
            port,
            0x0C,
            0x47,
            values.brightness_threshold,
        )
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x00, 0x0F, values.noise_threshold)
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x0C, 0x46, values.area_threshold_min)
        .await?;
    write_u16_reg(
        hub,
        device,
        port,
        0x00,
        [0x0B, 0x0C],
        values.area_threshold_max,
    )
    .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x00, 0x19, values.max_object_cnt)
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x00, 0x12, values.operation_mode)
        .await?;
    write_u16_reg(hub, device, port, 0x0C, [0x0F, 0x10], values.exposure_time).await?;
    write_u32_3byte(
        hub,
        device,
        port,
        0x0C,
        [0x07, 0x08, 0x09],
        values.frame_period,
    )
    .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x00, 0x28, values.frame_subtraction)
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x0C, 0x0B, gain.b_global)
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x0C, 0x0C, gain.b_ggh)
        .await?;
    write_u16_reg(hub, device, port, 0x0C, [0x60, 0x61], values.resolution_x).await?;
    write_u16_reg(hub, device, port, 0x0C, [0x62, 0x63], values.resolution_y).await?;
    // Sync
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x01, 0x01, 1)
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x00, 0x01, 1)
        .await?;
    Ok(())
}

pub(super) fn paj_load_defaults(port: u8) -> PajState {
    let is_nf = port == 0;
    PajState {
        brightness_threshold: "110".into(),
        noise_threshold: "15".into(),
        area_threshold_min: if is_nf { "10".into() } else { "5".into() },
        area_threshold_max: "9605".into(),
        max_object_cnt: "16".into(),
        operation_mode: "0".into(),
        exposure_time: if is_nf { "8192".into() } else { "11365".into() },
        frame_period: "49780".into(),
        frame_subtraction: "0".into(),
        gain_index: if is_nf {
            gain_index_from_reg(16, 0).to_string()
        } else {
            gain_index_from_reg(8, 3).to_string()
        },
        resolution_x: "4095".into(),
        resolution_y: "4095".into(),
    }
}

// Field names are shared between the validator and the panel so an issue can be
// routed back to the input that produced it.
pub(super) const PAJ_BRIGHTNESS: &str = "Brightness threshold";
pub(super) const PAJ_NOISE: &str = "Noise threshold";
pub(super) const PAJ_AREA_MIN: &str = "Area threshold min";
pub(super) const PAJ_AREA_MAX: &str = "Area threshold max";
pub(super) const PAJ_MAX_OBJECTS: &str = "Max object count";
pub(super) const PAJ_EXPOSURE: &str = "Exposure time";
pub(super) const PAJ_FRAME_PERIOD: &str = "Frame period";
pub(super) const PAJ_RESOLUTION_X: &str = "Resolution X";
pub(super) const PAJ_RESOLUTION_Y: &str = "Resolution Y";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::sensor_settings::field_issue::test_support::{issues, ok};

    #[test]
    fn paj_defaults_round_trip() {
        let v = ok(paj_validate(&paj_load_defaults(0)));
        assert_eq!(v.brightness_threshold, 110);
        assert_eq!(v.frame_period, 49780);
        assert_eq!(v.resolution_x, 4095);
        assert_eq!(v.gain_index, 16);
        let entry = &GAIN_TABLE[v.gain_index];
        assert_eq!(
            gain_index_from_reg(entry.b_global, entry.b_ggh),
            v.gain_index
        );

        let mut bad = paj_load_defaults(0);
        bad.brightness_threshold = "300".into();
        bad.resolution_y = "9000".into();
        let fields: Vec<_> = issues(paj_validate(&bad)).iter().map(|i| i.field).collect();
        assert_eq!(fields, vec![PAJ_BRIGHTNESS, PAJ_RESOLUTION_Y]);
    }
}
