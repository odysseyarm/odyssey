use odyssey_hub_client::client::DeviceClient;

use super::field_issue::{FieldChecker, FieldIssue};
use super::registers::{NEAR_FIELD_PORT, Register, RegisterRun, Registers};

// PAG7665 register map (used for product_id 0x5211).
const FPS: Register = Register::new(0x00, 0x13);
const EXPOSURE: Register = Register::new(0x00, 0x67);
const GAIN: Register = Register::new(0x00, 0x68);
const LIGHT_THRESHOLD: Register = Register::new(0x00, 0x6D);
const AREA_LOWER: RegisterRun<2> = RegisterRun::new(0x00, [0x6E, 0x6F]);
const AREA_UPPER: RegisterRun<2> = RegisterRun::new(0x00, [0x70, 0x71]);

// The exposure register holds 100us units in its low 7 bits and the
// "LED always on" flag in bit 7.
const EXPOSURE_UNIT_US: u16 = 100;
const EXPOSURE_UNITS_MASK: u8 = 0x7F;
const LED_ALWAYS_ON_FLAG: u8 = 0x80;

// The exposure register's 7 bits of 100us units make 12700us the ceiling;
// anything finer than 100us is truncated by the device.
pub(super) const PAG_MAX_EXPOSURE_US: u16 = 12_700;

#[derive(Clone, PartialEq, Default)]
pub(super) struct PagState {
    pub(super) fps: String,
    pub(super) exposure_us: String,
    pub(super) gain: String,
    pub(super) light_threshold: String,
    pub(super) area_lower: String,
    pub(super) area_upper: String,
}

fn decode_exposure_us(raw: u8) -> u16 {
    u16::from(raw & EXPOSURE_UNITS_MASK) * EXPOSURE_UNIT_US
}

/// Exposure register value for `exposure_us`, with the LED forced on.
fn encode_exposure(exposure_us: u16) -> u8 {
    // Validation caps `exposure_us` at PAG_MAX_EXPOSURE_US, which fits the mask.
    let units = (exposure_us / EXPOSURE_UNIT_US) as u8;
    units | LED_ALWAYS_ON_FLAG
}

pub(super) async fn pag_load_from_device(client: &DeviceClient) -> eros::Result<PagState> {
    let registers = Registers::new(client, NEAR_FIELD_PORT);
    let fps = registers.read(FPS).await?;
    let exposure_raw = registers.read(EXPOSURE).await?;
    let gain = registers.read(GAIN).await?;
    let light_threshold = registers.read(LIGHT_THRESHOLD).await?;
    let area_lower = registers.read_u16(AREA_LOWER).await?;
    let area_upper = registers.read_u16(AREA_UPPER).await?;

    Ok(PagState {
        fps: fps.to_string(),
        exposure_us: decode_exposure_us(exposure_raw).to_string(),
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

pub(super) fn pag_validate(state: &PagState) -> Result<PagValues, Vec<FieldIssue>> {
    let mut check = FieldChecker::default();
    let fps = check.check_u8(PAG_FPS, &state.fps);
    let exposure_us = check.check_u16_up_to(PAG_EXPOSURE, &state.exposure_us, PAG_MAX_EXPOSURE_US);
    let gain = check.check_u8(PAG_GAIN, &state.gain);
    let light_threshold = check.check_u8(PAG_LIGHT, &state.light_threshold);
    let area_lower = check.check_u16(PAG_AREA_MIN, &state.area_lower);
    let area_upper = check.check_u16(PAG_AREA_MAX, &state.area_upper);

    let (
        Some(fps),
        Some(exposure_us),
        Some(gain),
        Some(light_threshold),
        Some(area_lower),
        Some(area_upper),
    ) = (
        fps,
        exposure_us,
        gain,
        light_threshold,
        area_lower,
        area_upper,
    )
    else {
        return Err(check.into_issues());
    };

    Ok(PagValues {
        fps,
        exposure_us,
        gain,
        light_threshold,
        area_lower,
        area_upper,
    })
}

pub(super) async fn pag_apply_to_device(
    client: &DeviceClient,
    values: &PagValues,
) -> eros::Result<()> {
    let registers = Registers::new(client, NEAR_FIELD_PORT);
    registers.write(FPS, values.fps).await?;
    registers
        .write(EXPOSURE, encode_exposure(values.exposure_us))
        .await?;
    registers.write(GAIN, values.gain).await?;
    registers
        .write(LIGHT_THRESHOLD, values.light_threshold)
        .await?;
    registers.write_u16(AREA_LOWER, values.area_lower).await?;
    registers.write_u16(AREA_UPPER, values.area_upper).await?;
    Ok(())
}

/// The exposure in microseconds as milliseconds, for display next to the field.
pub(super) fn exposure_hint(raw: &str) -> String {
    let exposure_us: f64 = raw.parse().unwrap_or(0.0);
    format!("{:.1} ms", exposure_us / 1000.0)
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
    fn pag_exposure_is_bounded_and_encoded_in_100us_units() {
        let mut state = pag_load_defaults();
        for (exposure_us, encoded, decoded_us) in [
            (0, 0x80, 0),
            (99, 0x80, 0),
            (100, 0x81, 100),
            (12_700, 0xFF, 12_700),
        ] {
            state.exposure_us = exposure_us.to_string();
            let values = ok(pag_validate(&state));
            assert_eq!(encode_exposure(values.exposure_us), encoded);
            assert_eq!(decode_exposure_us(encoded), decoded_us);
            assert_eq!(decode_exposure_us(encoded & 0x7F), decoded_us);
        }
        state.exposure_us = "12701".into();
        assert_eq!(issues(pag_validate(&state))[0].field, PAG_EXPOSURE);
    }

    #[test]
    fn exposure_hint_shows_milliseconds() {
        assert_eq!(exposure_hint("2000"), "2.0 ms");
    }
}
