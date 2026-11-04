use std::time::Duration;

use anyhow::{Context as _, Result};
use eframe::egui;
use odyssey_usb::device::VmDevice;
use odyssey_usb::packets::vm::Port;

use super::{FIELD_WIDTH, form_grid, form_row, register_value, retry_checked};

/// Parses a text field that `validate()` has already checked.
fn parse_field<T>(text: &str, name: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    text.parse().with_context(|| format!("invalid {name}"))
}

/// A single-line text field of the common form field width.
fn text_field(ui: &mut egui::Ui, text: &mut String) {
    ui.add(egui::TextEdit::singleline(text).desired_width(FIELD_WIDTH));
}

#[derive(Clone)]
pub(super) struct PajSensorSettingsForm {
    port: Port,
    pid: String,
    resolution_x: String,
    resolution_y: String,
    exposure_time: String,
    frame_period: String,
    brightness_threshold: String,
    noise_threshold: String,
    area_threshold_min: String,
    area_threshold_max: String,
    max_object_cnt: String,

    operation_mode: i32,
    frame_subtraction: i32,
    gain: usize,
}

impl PajSensorSettingsForm {
    pub(super) fn new(port: Port) -> Self {
        Self {
            port,
            pid: String::new(),
            resolution_x: String::new(),
            resolution_y: String::new(),
            exposure_time: String::new(),
            frame_period: String::new(),
            brightness_threshold: String::new(),
            noise_threshold: String::new(),
            area_threshold_min: String::new(),
            area_threshold_max: String::new(),
            max_object_cnt: String::new(),
            operation_mode: 0,
            frame_subtraction: 0,
            gain: 0,
        }
    }

    /// Draws the form. The caller decides whether it is enabled.
    pub(super) fn ui(&mut self, ui: &mut egui::Ui) {
        let exposure_time_ms = match self.exposure_time.parse::<u16>() {
            Ok(n) => format!("{:.4}", f64::from(n) * 200.0 / 1e6),
            Err(_) => "???".to_string(),
        };
        let (frame_period_ms, fps) = match self.frame_period.parse::<u32>() {
            Ok(n) => (
                format!("{:.4}", f64::from(n) / 1e4),
                format!("{:.2}", 1e7 / f64::from(n)),
            ),
            Err(_) => ("???".to_string(), "???".to_string()),
        };
        let id_salt = match self.port {
            Port::Nf => "nf_sensor_settings",
            Port::Wf => "wf_sensor_settings",
        };

        form_grid(id_salt).show(ui, |ui| {
            form_row(ui, "Product ID", |ui| {
                ui.label(egui::RichText::new(&self.pid).monospace());
            });
            form_row(ui, "DSP brightness threshold", |ui| {
                text_field(ui, &mut self.brightness_threshold);
            });
            form_row(ui, "DSP noise threshold", |ui| {
                text_field(ui, &mut self.noise_threshold);
            });
            form_row(ui, "DSP area threshold min", |ui| {
                text_field(ui, &mut self.area_threshold_min);
            });
            form_row(ui, "DSP area threshold max", |ui| {
                text_field(ui, &mut self.area_threshold_max);
            });
            form_row(ui, "DSP maximum object count", |ui| {
                text_field(ui, &mut self.max_object_cnt);
            });
            form_row(ui, "DSP operation mode", |ui| {
                egui::ComboBox::from_id_salt("operation_mode")
                    .width(FIELD_WIDTH)
                    .selected_text(match self.operation_mode {
                        0 => "Normal",
                        1 => "Tracking",
                        _ => "???",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.operation_mode, 0, "Normal");
                        ui.selectable_value(&mut self.operation_mode, 1, "Tracking");
                    });
            });
            form_row(ui, "Exposure time", |ui| {
                text_field(ui, &mut self.exposure_time);
                ui.weak(format!("× 200 ns = {exposure_time_ms} ms"));
            });
            form_row(ui, "Frame period", |ui| {
                text_field(ui, &mut self.frame_period);
                ui.weak(format!("{frame_period_ms} ms ({fps} fps)"));
            });
            form_row(ui, "Frame subtraction", |ui| {
                egui::ComboBox::from_id_salt("frame_subtraction")
                    .width(FIELD_WIDTH)
                    .selected_text(match self.frame_subtraction {
                        0 => "Off",
                        1 => "On",
                        _ => "???",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.frame_subtraction, 0, "Off");
                        ui.selectable_value(&mut self.frame_subtraction, 1, "On");
                    });
            });
            form_row(ui, "Gain", |ui| {
                egui::ComboBox::from_id_salt("gain")
                    .width(FIELD_WIDTH)
                    .selected_text(GAIN_TABLE.get(self.gain).map_or("???", |gain| gain.0))
                    .show_ui(ui, |ui| {
                        for (index, (label, _)) in GAIN_TABLE.iter().enumerate() {
                            ui.selectable_value(&mut self.gain, index, *label);
                        }
                    });
            });
            form_row(ui, "Scale resolution X", |ui| {
                text_field(ui, &mut self.resolution_x);
            });
            form_row(ui, "Scale resolution Y", |ui| {
                text_field(ui, &mut self.resolution_y);
            });
        });
    }

    pub(super) async fn load_from_device(device: &VmDevice, port: Port) -> Result<Self> {
        let timeout = Duration::from_millis(2000);
        let pid = retry_checked(|| device.product_id(port), timeout, 3).await?;
        let res_x = retry_checked(|| device.resolution_x(port), timeout, 3).await?;
        let res_y = retry_checked(|| device.resolution_y(port), timeout, 3).await?;
        let expo = retry_checked(|| device.exposure_time(port), timeout, 3).await?;
        let frame_period = retry_checked(|| device.frame_period(port), timeout, 3).await?;
        let brightness_threshold =
            retry_checked(|| device.brightness_threshold(port), timeout, 3).await?;
        let noise_threshold = retry_checked(|| device.noise_threshold(port), timeout, 3).await?;
        let area_threshold_min =
            retry_checked(|| device.area_threshold_min(port), timeout, 3).await?;
        let area_threshold_max =
            retry_checked(|| device.area_threshold_max(port), timeout, 3).await?;
        let max_object_cnt = retry_checked(|| device.max_object_cnt(port), timeout, 3).await?;
        let operation_mode = retry_checked(|| device.operation_mode(port), timeout, 3).await?;
        let frame_subtraction =
            retry_checked(|| device.frame_subtraction(port), timeout, 3).await?;
        let gain_1 = retry_checked(|| device.gain_1(port), timeout, 3).await?;
        let gain_2 = retry_checked(|| device.gain_2(port), timeout, 3).await?;

        Ok(Self {
            port,
            pid: format!("0x{pid:04x}"),
            resolution_x: res_x.to_string(),
            resolution_y: res_y.to_string(),
            exposure_time: expo.to_string(),
            frame_period: frame_period.to_string(),
            brightness_threshold: brightness_threshold.to_string(),
            noise_threshold: noise_threshold.to_string(),
            area_threshold_min: area_threshold_min.to_string(),
            area_threshold_max: area_threshold_max.to_string(),
            max_object_cnt: max_object_cnt.to_string(),
            operation_mode: i32::from(operation_mode),
            frame_subtraction: i32::from(frame_subtraction),
            gain: Gain::index_from_reg(gain_1, gain_2),
        })
    }

    pub(super) fn validate(&self, errors: &mut Vec<String>) {
        macro_rules! validators {
            ($($display:literal $reg:ident : $ty:ty $({ $( $check:expr_2021 ),* $(,)? })? ),* $(,)?) => {
                $(
                    let $reg = self.$reg.parse::<$ty>();
                    match &$reg {
                        Ok(_) => (),
                        Err(e) => errors.push(format!("{}: {}", $display, e)),
                    }
                )*
                if !errors.is_empty() {
                    return
                }
                // Every field was parsed successfully above; rebind the parsed values
                // for the checks. Fields without a check are only referenced here.
                $( let Ok($reg) = $reg else { return }; )*
                let _ = ($(&$reg,)*);
                $(
                    $($(
                        let (test_result, msg) = ($check)($reg);
                        if !test_result {
                            errors.push(format!("{}: {}", $display, msg));
                        }
                    )*)?
                )*
            }
        }
        validators! {
            "exposure time" exposure_time: u16 {
                |x| (x >= 100, "must be >= 20 µs"),
                |x| ((200..=i64::from(frame_period) - 27000).contains(&(i64::from(x)*2)), "must be between 20 µs and frame period − 2.7 ms"),
            },
            "frame period" frame_period: u32 { |x| (x >= 49780, "must be >= 4.978 ms") },
            "brightness threshold" brightness_threshold: u8,
            "noise threshold" noise_threshold: u8,
            "area threshold min" area_threshold_min: u8,
            "area threshold max" area_threshold_max: u16 { |x| (x < (1 << 14), "must be < 16384") },
            "max object count" max_object_cnt: u8 { |x| ((1..=16).contains(&x), "must be between 1 and 16") },
            "scale resolution X" resolution_x: u16 { |x| ((1..=4095).contains(&x), "must be between 1 and 4095") },
            "scale resolution Y" resolution_y: u16 { |x| ((1..=4095).contains(&x), "must be between 1 and 4095") },
        }
    }

    /// Make sure to call `validate()` before calling this method.
    pub(super) async fn apply(&self, device: &VmDevice) -> Result<()> {
        let gain = GAIN_TABLE
            .get(self.gain)
            .with_context(|| format!("gain index {} is out of range", self.gain))?
            .1;

        tokio::try_join!(
            device.set_resolution_x(self.port, parse_field(&self.resolution_x, "resolution X")?),
            device.set_resolution_y(self.port, parse_field(&self.resolution_y, "resolution Y")?),
            device.set_gain_1(self.port, gain.b_global),
            device.set_gain_2(self.port, gain.b_ggh),
            device.set_exposure_time(
                self.port,
                parse_field(&self.exposure_time, "exposure time")?
            ),
            device.set_brightness_threshold(
                self.port,
                parse_field(&self.brightness_threshold, "brightness threshold")?
            ),
            device.set_noise_threshold(
                self.port,
                parse_field(&self.noise_threshold, "noise threshold")?
            ),
            device.set_area_threshold_max(
                self.port,
                parse_field(&self.area_threshold_max, "area threshold max")?
            ),
            device.set_area_threshold_min(
                self.port,
                parse_field(&self.area_threshold_min, "area threshold min")?
            ),
            device.set_operation_mode(
                self.port,
                register_value(self.operation_mode, "operation mode")?
            ),
            device.set_max_object_cnt(
                self.port,
                parse_field(&self.max_object_cnt, "max object count")?
            ),
            device.set_frame_subtraction(
                self.port,
                register_value(self.frame_subtraction, "frame subtraction")?
            ),
            device.set_frame_period(self.port, parse_field(&self.frame_period, "frame period")?),
        )?;
        tokio::try_join!(
            device.set_bank1_sync_updated(self.port, 1),
            device.set_bank0_sync_updated(self.port, 1),
        )?;
        Ok(())
    }

    pub(super) fn clear(&mut self) {
        *self = Self::new(self.port);
    }

    pub(super) fn load_defaults(&mut self) {
        self.resolution_x = "4095".into();
        self.resolution_y = "4095".into();
        self.frame_period = "49780".into();
        self.brightness_threshold = "110".into();
        self.noise_threshold = "15".into();
        self.area_threshold_max = "9605".into();
        self.max_object_cnt = "16".into();

        self.operation_mode = 0;
        self.frame_subtraction = 0;

        match self.port {
            Port::Nf => {
                self.area_threshold_min = "10".into();
                self.exposure_time = "8192".into();
                self.gain = Gain::index_from_reg(16, 0);
            }
            Port::Wf => {
                self.area_threshold_min = "5".into();
                self.exposure_time = "11365".into();
                self.gain = Gain::index_from_reg(8, 3);
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Gain {
    b_ggh: u8,
    b_global: u8,
}

impl Gain {
    const fn new(b_global: u8, b_ggh: u8) -> Self {
        Self { b_ggh, b_global }
    }
}

impl Gain {
    fn index_from_reg(b_global: u8, mut b_ggh: u8) -> usize {
        // change b_ggh from 0,2,3 to 0,1,2
        b_ggh = b_ggh.saturating_sub(1);
        usize::from(b_ggh) * 16 + usize::from(b_global)
    }
}

// funny

const fn n_to_bstr(n: usize) -> [u8; 6] {
    [
        b'0' + (n / 10000) as u8,
        b'.',
        b'0' + (n / 1000 % 10) as u8,
        b'0' + (n / 100 % 10) as u8,
        b'0' + (n / 10 % 10) as u8,
        b'0' + (n % 10) as u8,
    ]
}

// static GAIN_TABLE: [(&'static str, Gain); 16 * 3] = [
// for j, k in zip([0, 2, 3], [1, 2, 4]):
//     for i in range(16):
//         value = (1 + i/16) * k
//         print(f'    ("{value:.4f}", Gain::new(0x{i:02x}, 0x{j:02x})),')
// ];

pub(super) static GAIN_TABLE: [(&str, Gain); 16 * 3 + 1] = {
    const BACKING: [[u8; 6]; 16 * 3] = {
        let mut v = [[0; 6]; 16 * 3];
        let mut i = 0;
        while i < 16 {
            v[i] = n_to_bstr(10000 + i * 10000 / 16);
            v[i + 16] = n_to_bstr((10000 + i * 10000 / 16) * 2);
            v[i + 32] = n_to_bstr((10000 + i * 10000 / 16) * 4);
            i += 1;
        }
        v
    };
    let mut table = [("", Gain::new(0, 0)); 16 * 3 + 1];
    let mut i = 0;
    while i < 16 * 3 {
        // `n_to_bstr` yields ASCII digits, so this cannot fail; if it ever did,
        // evaluating the table would be a compile error, not a runtime one.
        table[i].0 = match std::str::from_utf8(&BACKING[i]) {
            Ok(text) => text,
            Err(_) => panic!("n_to_bstr produced bytes that are not UTF-8"),
        };
        table[i].1 = Gain::new((i % 16) as u8, [0, 2, 3][i / 16]);
        i += 1;
    }
    table[16 * 3] = ("8.0000", Gain::new(16, 3));
    table
};
