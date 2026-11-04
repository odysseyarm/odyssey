use std::time::Duration;

use eframe::egui;
use eros::Result;
use odyssey_usb::device::{ProductId, VmDevice};
use odyssey_usb::packets::vm::Port;

use super::{FIELD_WIDTH, form_grid, form_row, register_value, retry_checked};

/// Exposure is written in steps of 100 µs, in a 7-bit register field.
const EXPOSURE_STEP_US: i32 = 100;
const EXPOSURE_MAX_US: i32 = 12_700;

/// The circle parameters are Q3.5 fixed point numbers.
const CIRCLE_FIXED_POINT_SCALE: f32 = 32.0;

#[derive(Clone, Default)]
pub(super) struct PagSensorSettingsForm {
    cid: String,
    pag7665: bool,
    led_always_on: bool,
    fps: i32,
    exposure_us: i32,
    gain: i32,
    area_threshold_min: i32,
    area_threshold_max: i32,
    light_threshold: i32,
    // Circle detection parameters (PAG7665QN)
    circle_error: Option<String>,
    circle_r_min: i32,
    circle_r_max: i32,
    circle_k_min: i32,
    circle_k_max: i32,
}

/// The effect of a click inside the form that the owner has to carry out.
pub(super) enum PagAction {
    None,
    /// Open integrated USB image inspection and camera calibration.
    CaptureImage,
}

impl PagSensorSettingsForm {
    /// Draws the form. The caller decides whether it is enabled.
    pub(super) fn ui(&mut self, ui: &mut egui::Ui) -> PagAction {
        let mut action = PagAction::None;
        form_grid("pag_sensor_settings").show(ui, |ui| {
            form_row(ui, "Chip ID", |ui| {
                ui.label(egui::RichText::new(&self.cid).monospace());
            });
            form_row(ui, "FPS", |ui| {
                ui.add(egui::DragValue::new(&mut self.fps).range(0..=255));
            });
            form_row(ui, "Exposure time", |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.exposure_us)
                        .range(0..=EXPOSURE_MAX_US)
                        .speed(EXPOSURE_STEP_US)
                        .suffix(" µs"),
                );
            });
            form_row(ui, "Gain", |ui| {
                ui.add(egui::DragValue::new(&mut self.gain).range(0..=255));
            });
            form_row(ui, "Light threshold", |ui| {
                ui.add(egui::DragValue::new(&mut self.light_threshold).range(0..=255));
            });
            form_row(ui, "Area threshold min", |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.area_threshold_min)
                        .range(0..=i32::from(u16::MAX)),
                );
            });
            form_row(ui, "Area threshold max", |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.area_threshold_max)
                        .range(0..=i32::from(u16::MAX)),
                );
            });
            if self.pag7665 && self.circle_error.is_none() {
                circle_row(ui, "Circle R min", &mut self.circle_r_min);
                circle_row(ui, "Circle R max", &mut self.circle_r_max);
                circle_row(ui, "Circle K min", &mut self.circle_k_min);
                circle_row(ui, "Circle K max", &mut self.circle_k_max);
            }
            if let Some(error) = &self.circle_error {
                form_row(ui, "Circle settings unavailable", |ui| {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                    ui.weak(
                        "These settings will not be written. Reload to try reading them again.",
                    );
                });
            }
            form_row(ui, "Image mode", |ui| {
                if ui
                    .add_sized([FIELD_WIDTH, 0.0], egui::Button::new("Capture"))
                    .clicked()
                {
                    action = PagAction::CaptureImage;
                }
            });
        });
        action
    }

    pub(super) async fn load_from_device(device: &VmDevice, product_id: u16) -> Result<Self> {
        let timeout = Duration::from_millis(2000);

        // Use the product already read with general settings. A failed second
        // property read must never silently select a different register map.
        let is_pag7665 = ProductId::from_u16(product_id) == Some(ProductId::AtsPro);

        let cid = retry_checked(|| device.pag_chip_id(Port::Nf), timeout, 3).await?;
        let fps = retry_checked(|| device.pag_fps(Port::Nf), timeout, 3).await?;

        // The PAG7665QN keeps these registers at different addresses from the PAG7661QN.
        let (expo, area_threshold_min, area_threshold_max, light_threshold, gain) = if is_pag7665 {
            let expo = retry_checked(|| device.pag7665_exposure(Port::Nf), timeout, 3).await?;
            let area_threshold_min =
                retry_checked(|| device.pag7665_area_lower(Port::Nf), timeout, 3).await?;
            let area_threshold_max =
                retry_checked(|| device.pag7665_area_upper(Port::Nf), timeout, 3).await?;
            let light_threshold =
                retry_checked(|| device.pag7665_light_threshold(Port::Nf), timeout, 3).await?;
            let gain = retry_checked(|| device.pag7665_gain(Port::Nf), timeout, 3).await?;
            (
                expo,
                area_threshold_min,
                area_threshold_max,
                light_threshold,
                gain,
            )
        } else {
            let expo = retry_checked(|| device.pag_exposure(Port::Nf), timeout, 3).await?;
            let area_threshold_min =
                retry_checked(|| device.pag_area_lower(Port::Nf), timeout, 3).await?;
            let area_threshold_max =
                retry_checked(|| device.pag_area_upper(Port::Nf), timeout, 3).await?;
            let light_threshold =
                retry_checked(|| device.pag_light_threshold(Port::Nf), timeout, 3).await?;
            let gain = retry_checked(|| device.pag_gain(Port::Nf), timeout, 3).await?;
            (
                expo,
                area_threshold_min,
                area_threshold_max,
                light_threshold,
                gain,
            )
        };

        let (circles, circle_error) = if is_pag7665 {
            let result: Result<_> = async {
                Ok((
                    retry_checked(|| device.pag_circle_r_min(Port::Nf), timeout, 3).await?,
                    retry_checked(|| device.pag_circle_r_max(Port::Nf), timeout, 3).await?,
                    retry_checked(|| device.pag_circle_k_min(Port::Nf), timeout, 3).await?,
                    retry_checked(|| device.pag_circle_k_max(Port::Nf), timeout, 3).await?,
                ))
            }
            .await;
            match result {
                Ok(values) => (values, None),
                Err(error) => {
                    tracing::warn!("Could not read optional circle settings: {error:#?}");
                    ((0, 0, 0, 0), Some(format!("{error:#?}")))
                }
            }
        } else {
            ((0, 0, 0, 0), None)
        };

        Ok(Self {
            cid: format!("0x{cid:04x}"),
            pag7665: is_pag7665,
            led_always_on: expo & (1 << 7) != 0,
            fps: i32::from(fps),
            exposure_us: i32::from(u16::from(expo & !(1 << 7)) * 100),
            gain: i32::from(gain),
            area_threshold_min: i32::from(area_threshold_min),
            area_threshold_max: i32::from(area_threshold_max),
            light_threshold: i32::from(light_threshold),
            circle_error,
            circle_r_min: i32::from(circles.0),
            circle_r_max: i32::from(circles.1),
            circle_k_min: i32::from(circles.2),
            circle_k_max: i32::from(circles.3),
        })
    }

    pub(super) fn validate(&self, errors: &mut Vec<String>) {
        for (name, value, max) in [
            ("fps", self.fps, 255),
            ("gain", self.gain, 255),
            ("light threshold", self.light_threshold, 255),
            (
                "area threshold min",
                self.area_threshold_min,
                i32::from(u16::MAX),
            ),
            (
                "area threshold max",
                self.area_threshold_max,
                i32::from(u16::MAX),
            ),
        ] {
            if !(0..=max).contains(&value) {
                errors.push(format!("{name}: must be between 0 and {max}"));
            }
        }
        if self.area_threshold_min > self.area_threshold_max {
            errors.push("area threshold min: must not exceed area threshold max".into());
        }
        if self.pag7665 && self.circle_error.is_none() {
            for (name, min, max) in [
                ("circle R", self.circle_r_min, self.circle_r_max),
                ("circle K", self.circle_k_min, self.circle_k_max),
            ] {
                if !(0..=255).contains(&min) || !(0..=255).contains(&max) {
                    errors.push(format!("{name}: values must be between 0 and 255"));
                }
                if min > max {
                    errors.push(format!("{name} min: must not exceed max"));
                }
            }
        }
        if self.exposure_us % EXPOSURE_STEP_US != 0 {
            errors.push(format!(
                "exposure time: must be a multiple of {EXPOSURE_STEP_US} µs"
            ));
        }
        if !(0..=EXPOSURE_MAX_US).contains(&self.exposure_us) {
            errors.push(format!(
                "exposure time: must be between 0 and {EXPOSURE_MAX_US} µs"
            ));
        }
    }

    /// Run `validate()` first.
    pub(super) async fn apply(&self, device: &VmDevice) -> Result<()> {
        let is_pag7665 = self.pag7665;

        let fps = register_value(self.fps, "fps")?;

        let gain = register_value(self.gain, "gain")?;

        let exposure = self.exposure_register()?;

        let area_threshold_min = register_value(self.area_threshold_min, "area threshold min")?;
        let area_threshold_max = register_value(self.area_threshold_max, "area threshold max")?;
        let light_threshold = register_value(self.light_threshold, "light threshold")?;

        let circles = self.circle_registers()?;

        // The PAG7665QN keeps these registers at different addresses from the PAG7661QN.
        if is_pag7665 {
            tokio::try_join!(
                device.set_pag_fps(Port::Nf, fps),
                device.set_pag7665_gain(Port::Nf, gain),
                device.set_pag7665_exposure(Port::Nf, exposure),
                device.set_pag7665_area_lower(Port::Nf, area_threshold_min),
                device.set_pag7665_area_upper(Port::Nf, area_threshold_max),
                device.set_pag7665_light_threshold(Port::Nf, light_threshold),
            )?;
        } else {
            tokio::try_join!(
                device.set_pag_fps(Port::Nf, fps),
                device.set_pag_gain(Port::Nf, gain),
                device.set_pag_exposure(Port::Nf, exposure),
                device.set_pag_area_lower(Port::Nf, area_threshold_min),
                device.set_pag_area_upper(Port::Nf, area_threshold_max),
                device.set_pag_light_threshold(Port::Nf, light_threshold),
            )?;
        }
        if let Some([r_min, r_max, k_min, k_max]) = circles {
            tokio::try_join!(
                device.set_pag_circle_r_min(Port::Nf, r_min),
                device.set_pag_circle_r_max(Port::Nf, r_max),
                device.set_pag_circle_k_min(Port::Nf, k_min),
                device.set_pag_circle_k_max(Port::Nf, k_max),
            )?;
        }
        Ok(())
    }

    fn exposure_register(&self) -> Result<u8> {
        Ok(
            register_value::<u8>(self.exposure_us / EXPOSURE_STEP_US, "exposure time")?
                | (u8::from(self.led_always_on) << 7),
        )
    }

    fn circle_registers(&self) -> Result<Option<[u8; 4]>> {
        if !self.pag7665 || self.circle_error.is_some() {
            return Ok(None);
        }
        Ok(Some([
            register_value(self.circle_r_min, "circle R min")?,
            register_value(self.circle_r_max, "circle R max")?,
            register_value(self.circle_k_min, "circle K min")?,
            register_value(self.circle_k_max, "circle K max")?,
        ]))
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn load_defaults(&mut self) {
        self.fps = 180;
        self.exposure_us = 2000;
        self.led_always_on = true;
        self.gain = 2;
        self.area_threshold_min = 10;
        self.area_threshold_max = u16::MAX.into();
        self.light_threshold = 120;

        // Circle detection defaults (from PAG7665QN datasheet)
        // Q3.5 format: value / 32.0 = float
        self.circle_r_min = 0x1A; // 26 → 0.8125
        self.circle_r_max = 0x26; // 38 → 1.1875
        self.circle_k_min = 0x17; // 23 → 0.71875
        self.circle_k_max = 0x1D; // 29 → 0.90625
    }
}

/// A Q3.5 value with its decimal reading next to it.
fn circle_row(ui: &mut egui::Ui, label: &str, value: &mut i32) {
    form_row(ui, label, |ui| {
        ui.add(egui::DragValue::new(value).range(0..=255));
        ui.weak(format!("= {:.3}", *value as f32 / CIRCLE_FIXED_POINT_SCALE));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposure_preserves_led_bit_at_register_boundary() {
        let mut form = PagSensorSettingsForm {
            exposure_us: EXPOSURE_MAX_US,
            ..Default::default()
        };
        assert_eq!(form.exposure_register().unwrap(), 0x7f);
        form.led_always_on = true;
        assert_eq!(form.exposure_register().unwrap(), 0xff);
    }

    #[test]
    fn exposure_validation_rejects_rounding_and_register_overflow() {
        for exposure_us in [-100, 99, 12_701, 12_800] {
            let form = PagSensorSettingsForm {
                exposure_us,
                ..Default::default()
            };
            let mut errors = Vec::new();
            form.validate(&mut errors);
            assert!(
                errors
                    .iter()
                    .any(|error| error.starts_with("exposure time")),
                "{exposure_us}"
            );
        }
    }

    #[test]
    fn threshold_ranges_must_be_ordered() {
        let form = PagSensorSettingsForm {
            pag7665: true,
            area_threshold_min: 11,
            area_threshold_max: 10,
            circle_r_min: 27,
            circle_r_max: 26,
            circle_k_min: 24,
            circle_k_max: 23,
            ..Default::default()
        };
        let mut errors = Vec::new();
        form.validate(&mut errors);
        for field in ["area threshold min", "circle R min", "circle K min"] {
            assert!(
                errors.iter().any(|error| error.starts_with(field)),
                "{field}"
            );
        }
    }

    #[test]
    fn unavailable_circle_settings_are_not_encoded_for_writing() {
        let mut form = PagSensorSettingsForm {
            pag7665: true,
            circle_error: Some("Read timed out".into()),
            ..Default::default()
        };
        form.load_defaults();
        assert_eq!(form.circle_registers().unwrap(), None);
    }
}
