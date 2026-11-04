use std::time::Duration;

use anyhow::Result;
use eframe::egui;
use odyssey_usb::device::{ProductId, VmDevice};
use odyssey_usb::packets::vm::{Mode, Port, PropKind, Props};

use super::{FIELD_WIDTH, form_grid, form_row, register_value, retry, retry_checked};

/// Exposure is written in steps of 100 µs, in a 7-bit register field.
const EXPOSURE_STEP_US: i32 = 100;
const EXPOSURE_MAX_US: i32 = 12_700;

/// The circle parameters are Q3.5 fixed point numbers.
const CIRCLE_FIXED_POINT_SCALE: f32 = 32.0;

#[derive(Clone, Default)]
pub(super) struct PagSensorSettingsForm {
    cid: String,
    fps: i32,
    exposure_us: i32,
    gain: i32,
    area_threshold_min: i32,
    area_threshold_max: i32,
    light_threshold: i32,
    // Circle detection parameters (PAG7665QN)
    circle_r_min: i32,
    circle_r_max: i32,
    circle_k_min: i32,
    circle_k_max: i32,
}

/// The effect of a click inside the form that the owner has to carry out.
pub(super) enum PagAction {
    None,
    /// Switch the sensor to image mode.
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
            circle_row(ui, "Circle R min", &mut self.circle_r_min);
            circle_row(ui, "Circle R max", &mut self.circle_r_max);
            circle_row(ui, "Circle K min", &mut self.circle_k_min);
            circle_row(ui, "Circle K max", &mut self.circle_k_max);
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

    pub(super) async fn load_from_device(device: &VmDevice) -> Result<Self> {
        let timeout = Duration::from_millis(2000);

        // Check product ID to determine PAG variant (PAG7665QN vs PAG7661QN)
        let product_id = retry(|| device.read_prop(PropKind::ProductId), timeout, 3)
            .await
            .and_then(std::result::Result::ok)
            .and_then(|p| match p {
                Props::ProductId(id) => ProductId::from_u16(id),
                _ => None,
            });
        let is_pag7665 = product_id == Some(ProductId::AtsPro);

        let cid = retry_checked(|| device.pag_chip_id(Port::Nf), timeout, 3).await?;
        let fps = retry_checked(|| device.pag_fps(Port::Nf), timeout, 3).await?;

        // Use correct register addresses based on PAG variant
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
        let _led_always_on = expo & (1 << 7) != 0;

        // Circle detection parameters (PAG7665QN)
        let circle_r_min = retry(|| device.pag_circle_r_min(Port::Nf), timeout, 3)
            .await
            .unwrap_or(Ok(0))?;
        let circle_r_max = retry(|| device.pag_circle_r_max(Port::Nf), timeout, 3)
            .await
            .unwrap_or(Ok(255))?;
        let circle_k_min = retry(|| device.pag_circle_k_min(Port::Nf), timeout, 3)
            .await
            .unwrap_or(Ok(0))?;
        let circle_k_max = retry(|| device.pag_circle_k_max(Port::Nf), timeout, 3)
            .await
            .unwrap_or(Ok(255))?;

        Ok(Self {
            cid: format!("0x{cid:04x}"),
            fps: i32::from(fps),
            exposure_us: i32::from(u16::from(expo & !(1 << 7)) * 100),
            gain: i32::from(gain),
            area_threshold_min: i32::from(area_threshold_min),
            area_threshold_max: i32::from(area_threshold_max),
            light_threshold: i32::from(light_threshold),
            circle_r_min: i32::from(circle_r_min),
            circle_r_max: i32::from(circle_r_max),
            circle_k_min: i32::from(circle_k_min),
            circle_k_max: i32::from(circle_k_max),
        })
    }

    /// Every setting but the exposure is a ranged drag value that already keeps
    /// it valid. The exposure can still be typed in as any number.
    pub(super) fn validate(&self, errors: &mut Vec<String>) {
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

    /// Make sure to call `validate()` before calling this method.
    pub(super) async fn apply(&self, device: &VmDevice) -> Result<()> {
        // Check product ID to determine PAG variant (PAG7665QN vs PAG7661QN)
        let product_id = device
            .read_prop(PropKind::ProductId)
            .await
            .ok()
            .and_then(|p| match p {
                Props::ProductId(id) => ProductId::from_u16(id),
                _ => None,
            });
        let is_pag7665 = product_id == Some(ProductId::AtsPro);

        let fps = register_value(self.fps, "fps")?;

        let gain = register_value(self.gain, "gain")?;

        // TODO
        let led_always_on = true;
        let exposure = register_value::<u8>(self.exposure_us / EXPOSURE_STEP_US, "exposure time")?
            | (u8::from(led_always_on) << 7);

        let area_threshold_min = register_value(self.area_threshold_min, "area threshold min")?;
        let area_threshold_max = register_value(self.area_threshold_max, "area threshold max")?;
        let light_threshold = register_value(self.light_threshold, "light threshold")?;

        let circle_r_min = register_value(self.circle_r_min, "circle R min")?;
        let circle_r_max = register_value(self.circle_r_max, "circle R max")?;
        let circle_k_min = register_value(self.circle_k_min, "circle K min")?;
        let circle_k_max = register_value(self.circle_k_max, "circle K max")?;

        // Use correct register addresses based on PAG variant
        if is_pag7665 {
            tokio::try_join!(
                device.set_pag_fps(Port::Nf, fps),
                device.set_pag7665_gain(Port::Nf, gain),
                device.set_pag7665_exposure(Port::Nf, exposure),
                device.set_pag7665_area_lower(Port::Nf, area_threshold_min),
                device.set_pag7665_area_upper(Port::Nf, area_threshold_max),
                device.set_pag7665_light_threshold(Port::Nf, light_threshold),
                device.set_pag_circle_r_min(Port::Nf, circle_r_min),
                device.set_pag_circle_r_max(Port::Nf, circle_r_max),
                device.set_pag_circle_k_min(Port::Nf, circle_k_min),
                device.set_pag_circle_k_max(Port::Nf, circle_k_max),
            )?;
        } else {
            tokio::try_join!(
                device.set_pag_fps(Port::Nf, fps),
                device.set_pag_gain(Port::Nf, gain),
                device.set_pag_exposure(Port::Nf, exposure),
                device.set_pag_area_lower(Port::Nf, area_threshold_min),
                device.set_pag_area_upper(Port::Nf, area_threshold_max),
                device.set_pag_light_threshold(Port::Nf, light_threshold),
                device.set_pag_circle_r_min(Port::Nf, circle_r_min),
                device.set_pag_circle_r_max(Port::Nf, circle_r_max),
                device.set_pag_circle_k_min(Port::Nf, circle_k_min),
                device.set_pag_circle_k_max(Port::Nf, circle_k_max),
            )?;
        }
        Ok(())
    }

    /// Switches the sensor to image mode; failures are only logged, as before.
    pub(super) async fn capture_image(device: &VmDevice) {
        let timeout = Duration::from_millis(2000);
        if let Err(e) = retry_checked(|| device.write_mode(Mode::Image), timeout, 3).await {
            tracing::error!("Failed to switch to image mode: {e:#}");
        }
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn load_defaults(&mut self) {
        self.fps = 180;
        self.exposure_us = 2000;
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
