use odyssey_hub_client::client::DeviceClient;

use super::field_issue::{FieldChecker, FieldIssue};
use super::gain_table::{GAIN_TABLE, gain_index_from_reg};
use super::registers::{Register, RegisterRun, Registers};

// PAJ sensor register map. Exposure and gain are read back from different
// addresses than they are written to; both are kept as the device defines them.
const BRIGHTNESS_THRESHOLD: Register = Register::new(0x0C, 0x47);
const NOISE_THRESHOLD: Register = Register::new(0x00, 0x0F);
const AREA_THRESHOLD_MIN: Register = Register::new(0x0C, 0x46);
const AREA_THRESHOLD_MAX: RegisterRun<2> = RegisterRun::new(0x00, [0x0B, 0x0C]);
const MAX_OBJECT_COUNT: Register = Register::new(0x00, 0x19);
const OPERATION_MODE: Register = Register::new(0x00, 0x12);
const EXPOSURE_TIME_READ: RegisterRun<2> = RegisterRun::new(0x01, [0x0E, 0x0F]);
const EXPOSURE_TIME_WRITE: RegisterRun<2> = RegisterRun::new(0x0C, [0x0F, 0x10]);
const FRAME_PERIOD: RegisterRun<3> = RegisterRun::new(0x0C, [0x07, 0x08, 0x09]);
const FRAME_SUBTRACTION: Register = Register::new(0x00, 0x28);
const GAIN_GLOBAL_READ: Register = Register::new(0x01, 0x05);
const GAIN_GGH_READ: Register = Register::new(0x01, 0x06);
const GAIN_GLOBAL_WRITE: Register = Register::new(0x0C, 0x0B);
const GAIN_GGH_WRITE: Register = Register::new(0x0C, 0x0C);
const RESOLUTION_X: RegisterRun<2> = RegisterRun::new(0x0C, [0x60, 0x61]);
const RESOLUTION_Y: RegisterRun<2> = RegisterRun::new(0x0C, [0x62, 0x63]);
/// Latch registers; writing 1 to both applies the staged values.
const SYNC_BANK_1: Register = Register::new(0x01, 0x01);
const SYNC_BANK_0: Register = Register::new(0x00, 0x01);

const MAX_OBJECT_COUNT_LIMIT: u8 = 16;
const RESOLUTION_LIMIT: u16 = 4095;
/// The frame period register is 24 bits wide.
const FRAME_PERIOD_LIMIT: u32 = 0xFF_FFFF;

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
    client: &DeviceClient,
    port: u8,
) -> eros::Result<PajState> {
    let registers = Registers::new(client, port);
    let brightness_threshold = registers.read(BRIGHTNESS_THRESHOLD).await?;
    let noise_threshold = registers.read(NOISE_THRESHOLD).await?;
    let area_threshold_min = registers.read(AREA_THRESHOLD_MIN).await?;
    let area_threshold_max = registers.read_u16(AREA_THRESHOLD_MAX).await?;
    let max_object_cnt = registers.read(MAX_OBJECT_COUNT).await?;
    let operation_mode = registers.read(OPERATION_MODE).await?;
    let exposure_time = registers.read_u16(EXPOSURE_TIME_READ).await?;
    let frame_period = registers.read_u24(FRAME_PERIOD).await?;
    let frame_subtraction = registers.read(FRAME_SUBTRACTION).await?;
    let gain_global = registers.read(GAIN_GLOBAL_READ).await?;
    let gain_ggh = registers.read(GAIN_GGH_READ).await?;
    let gain_index = gain_index_from_reg(gain_global, gain_ggh).ok_or_else(|| {
        eros::error!("Unsupported PAJ gain registers: global={gain_global}, ggh={gain_ggh}")
    })?;
    let resolution_x = registers.read_u16(RESOLUTION_X).await?;
    let resolution_y = registers.read_u16(RESOLUTION_Y).await?;

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
        gain_index: gain_index.to_string(),
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
    let mut check = FieldChecker::default();
    let brightness = check.check_u8(PAJ_BRIGHTNESS, &state.brightness_threshold);
    let noise = check.check_u8(PAJ_NOISE, &state.noise_threshold);
    let area_min = check.check_u8(PAJ_AREA_MIN, &state.area_threshold_min);
    let area_max = check.check_u16(PAJ_AREA_MAX, &state.area_threshold_max);
    let max_objects = check.check_u8_up_to(
        PAJ_MAX_OBJECTS,
        &state.max_object_cnt,
        MAX_OBJECT_COUNT_LIMIT,
    );
    let exposure = check.check_u16(PAJ_EXPOSURE, &state.exposure_time);
    let frame_period = check.check(PAJ_FRAME_PERIOD, &state.frame_period, FRAME_PERIOD_LIMIT);
    let resolution_x =
        check.check_u16_up_to(PAJ_RESOLUTION_X, &state.resolution_x, RESOLUTION_LIMIT);
    let resolution_y =
        check.check_u16_up_to(PAJ_RESOLUTION_Y, &state.resolution_y, RESOLUTION_LIMIT);
    let operation_mode = check.check_u8_up_to(PAJ_OPERATION_MODE, &state.operation_mode, 1);
    let frame_subtraction =
        check.check_u8_up_to(PAJ_FRAME_SUBTRACTION, &state.frame_subtraction, 1);
    let gain_index = check.check(PAJ_GAIN, &state.gain_index, (GAIN_TABLE.len() - 1) as u32);

    let (
        Some(brightness_threshold),
        Some(noise_threshold),
        Some(area_threshold_min),
        Some(area_threshold_max),
        Some(max_object_cnt),
        Some(exposure_time),
        Some(frame_period),
        Some(resolution_x),
        Some(resolution_y),
        Some(operation_mode),
        Some(frame_subtraction),
        Some(gain_index),
    ) = (
        brightness,
        noise,
        area_min,
        area_max,
        max_objects,
        exposure,
        frame_period,
        resolution_x,
        resolution_y,
        operation_mode,
        frame_subtraction,
        gain_index,
    )
    else {
        return Err(check.into_issues());
    };

    Ok(PajValues {
        brightness_threshold,
        noise_threshold,
        area_threshold_min,
        area_threshold_max,
        max_object_cnt,
        operation_mode,
        exposure_time,
        frame_period,
        frame_subtraction,
        gain_index: gain_index as usize,
        resolution_x,
        resolution_y,
    })
}

pub(super) async fn paj_apply_to_device(
    client: &DeviceClient,
    port: u8,
    values: &PajValues,
) -> eros::Result<()> {
    let registers = Registers::new(client, port);
    let gain = &GAIN_TABLE[values.gain_index];

    registers
        .write(BRIGHTNESS_THRESHOLD, values.brightness_threshold)
        .await?;
    registers
        .write(NOISE_THRESHOLD, values.noise_threshold)
        .await?;
    registers
        .write(AREA_THRESHOLD_MIN, values.area_threshold_min)
        .await?;
    registers
        .write_u16(AREA_THRESHOLD_MAX, values.area_threshold_max)
        .await?;
    registers
        .write(MAX_OBJECT_COUNT, values.max_object_cnt)
        .await?;
    registers
        .write(OPERATION_MODE, values.operation_mode)
        .await?;
    registers
        .write_u16(EXPOSURE_TIME_WRITE, values.exposure_time)
        .await?;
    registers
        .write_u24(FRAME_PERIOD, values.frame_period)
        .await?;
    registers
        .write(FRAME_SUBTRACTION, values.frame_subtraction)
        .await?;
    registers.write(GAIN_GLOBAL_WRITE, gain.b_global).await?;
    registers.write(GAIN_GGH_WRITE, gain.b_ggh).await?;
    registers
        .write_u16(RESOLUTION_X, values.resolution_x)
        .await?;
    registers
        .write_u16(RESOLUTION_Y, values.resolution_y)
        .await?;
    registers.write(SYNC_BANK_1, 1).await?;
    registers.write(SYNC_BANK_0, 1).await?;
    Ok(())
}

/// One exposure tick is 200 ns.
const EXPOSURE_TICK_NS: f64 = 200.0;
const NS_PER_MS: f64 = 1e6;
/// The frame period counts 100 ns ticks.
const FRAME_PERIOD_TICKS_PER_MS: f64 = 1e4;
const FRAME_PERIOD_TICKS_PER_SECOND: f64 = 1e7;

/// The raw exposure ticks as milliseconds, for display next to the field.
pub(super) fn exposure_hint(raw: &str) -> String {
    let ticks: f64 = raw.parse().unwrap_or(0.0);
    format!("{:.2} ms", ticks * EXPOSURE_TICK_NS / NS_PER_MS)
}

/// The raw frame period as milliseconds and frames per second.
pub(super) fn frame_period_hint(raw: &str) -> String {
    let ticks: f64 = raw.parse().unwrap_or(0.0);
    if ticks > 0.0 {
        format!(
            "{:.2} ms ({:.1} fps)",
            ticks / FRAME_PERIOD_TICKS_PER_MS,
            FRAME_PERIOD_TICKS_PER_SECOND / ticks
        )
    } else {
        "---".into()
    }
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
        gain_index: if is_nf { "16".into() } else { "40".into() },
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
pub(super) const PAJ_OPERATION_MODE: &str = "Operation mode";
pub(super) const PAJ_FRAME_SUBTRACTION: &str = "Frame subtraction";
pub(super) const PAJ_GAIN: &str = "Gain";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::sensor_settings::field_issue::test_support::{issues, ok};

    #[test]
    fn paj_validation_rejects_out_of_range_fields() {
        let mut bad = paj_load_defaults(0);
        bad.brightness_threshold = "300".into();
        bad.resolution_y = "9000".into();
        let fields: Vec<_> = issues(paj_validate(&bad)).iter().map(|i| i.field).collect();
        assert_eq!(fields, vec![PAJ_BRIGHTNESS, PAJ_RESOLUTION_Y]);
    }

    #[test]
    fn dropdown_values_are_validated_instead_of_defaulted_or_clamped() {
        let mut state = paj_load_defaults(0);
        state.operation_mode = "2".into();
        state.frame_subtraction = "invalid".into();
        state.gain_index = GAIN_TABLE.len().to_string();
        let found = issues(paj_validate(&state));
        let fields: Vec<_> = found.iter().map(|issue| issue.field).collect();
        assert_eq!(
            fields,
            [PAJ_OPERATION_MODE, PAJ_FRAME_SUBTRACTION, PAJ_GAIN]
        );

        state.operation_mode = "1".into();
        state.frame_subtraction = "1".into();
        state.gain_index = (GAIN_TABLE.len() - 1).to_string();
        let values = ok(paj_validate(&state));
        assert_eq!(values.operation_mode, 1);
        assert_eq!(values.frame_subtraction, 1);
        assert_eq!(values.gain_index, GAIN_TABLE.len() - 1);
    }

    #[test]
    fn hints_convert_ticks_to_engineering_units() {
        assert_eq!(exposure_hint("8192"), "1.64 ms");
        assert_eq!(frame_period_hint("49780"), "4.98 ms (200.9 fps)");
        assert_eq!(frame_period_hint("0"), "---");
        assert_eq!(frame_period_hint("oops"), "---");
    }
}
