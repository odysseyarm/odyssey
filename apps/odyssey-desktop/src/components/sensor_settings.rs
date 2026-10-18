use crate::hub::HubContext;
use dioxus::{logger::tracing, prelude::*};
use dioxus::prelude::UnboundedReceiver;
use futures::StreamExt;
use odyssey_hub_common::device::{Device, DeviceCapabilities};

// Product ID constants
const PID_ATS_VM: u16 = 0x520F;
const PID_ATS_LITE: u16 = 0x5210;
const PID_ATS_PRO: u16 = 0x5211;

// --- Status reporting ---

/// Outcome of the most recent user-triggered operation in a panel.
///
/// `Failed` keeps the customer-facing sentence separate from the raw error so
/// the latter can stay collapsed; support still gets the exact text.
#[derive(Clone, PartialEq, Default)]
enum OpStatus {
    #[default]
    Idle,
    Busy(String),
    Done(String),
    Failed {
        summary: String,
        detail: Option<String>,
    },
}

/// Turn a transport/RPC failure into something a customer can act on.
///
/// The hub reports an unusable events transport as a plain `Internal` error;
/// surfaced verbatim it reads like a crash, when in fact the device is simply
/// in BLE mode and the register plane lives on the USB bulk endpoint.
fn rpc_failure(context: &str, err: &dyn std::fmt::Display) -> OpStatus {
    let raw = format!("{err:#}");
    let summary = if raw.contains("events transport not connected") {
        "Sensor tuning needs the USB data connection. Switch Transport mode to \
         USB in the Connection section, then try again."
            .to_string()
    } else if raw.contains("not connected to the hub") || raw.contains("transport error") {
        "Lost the connection to the Odyssey service.".to_string()
    } else if raw.contains("device not found") {
        "The device disconnected before the request finished.".to_string()
    } else {
        format!("Could not {context}.")
    };
    OpStatus::Failed {
        summary,
        detail: Some(raw),
    }
}

#[component]
fn StatusLine(status: OpStatus) -> Element {
    match status {
        OpStatus::Idle => rsx! {},
        OpStatus::Busy(msg) => rsx! {
            span { class: "text-xs text-gray-500 dark:text-gray-400 italic", "{msg}" }
        },
        OpStatus::Done(msg) => rsx! {
            span { class: "text-xs text-green-600 dark:text-green-400", "{msg}" }
        },
        OpStatus::Failed { summary, detail } => rsx! {
            div {
                class: "text-xs text-red-600 dark:text-red-400 space-y-1 basis-full",
                p { "{summary}" }
                if let Some(detail) = detail {
                    details {
                        summary {
                            class: "cursor-pointer text-gray-500 dark:text-gray-400 select-none",
                            "Technical details"
                        }
                        p {
                            class: "mt-1 font-mono text-[11px] text-gray-500 dark:text-gray-400 break-all",
                            "{detail}"
                        }
                    }
                }
            }
        },
    }
}

// --- Field validation ---

/// A value the user typed that cannot be sent to the device.
#[derive(Clone, PartialEq)]
struct FieldIssue {
    field: &'static str,
    message: String,
}

/// Parse an unsigned field and bound it, so a typo is reported instead of
/// silently turning into a default that gets written to the sensor.
fn parse_bounded(field: &'static str, raw: &str, max: u32) -> Result<u32, FieldIssue> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(FieldIssue {
            field,
            message: "Required".into(),
        });
    }
    match raw.parse::<u32>() {
        Ok(v) if v <= max => Ok(v),
        Ok(_) => Err(FieldIssue {
            field,
            message: format!("Must be 0-{max}"),
        }),
        Err(_) => Err(FieldIssue {
            field,
            message: format!("Whole number 0-{max}"),
        }),
    }
}

fn issue_for<'a>(issues: &'a [FieldIssue], field: &str) -> Option<&'a str> {
    issues
        .iter()
        .find(|i| i.field == field)
        .map(|i| i.message.as_str())
}

// --- Multi-byte register helpers ---

async fn read_u16_reg(
    hub: &HubContext,
    device: &Device,
    port: u8,
    bank: u8,
    addrs: [u8; 2],
) -> anyhow::Result<u16> {
    let lo = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, bank, addrs[0])
        .await?;
    let hi = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, bank, addrs[1])
        .await?;
    Ok(u16::from_le_bytes([lo, hi]))
}

async fn write_u16_reg(
    hub: &HubContext,
    device: &Device,
    port: u8,
    bank: u8,
    addrs: [u8; 2],
    val: u16,
) -> anyhow::Result<()> {
    let bytes = val.to_le_bytes();
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, bank, addrs[0], bytes[0])
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, bank, addrs[1], bytes[1])
        .await?;
    Ok(())
}

async fn read_u32_3byte(
    hub: &HubContext,
    device: &Device,
    port: u8,
    bank: u8,
    addrs: [u8; 3],
) -> anyhow::Result<u32> {
    let b0 = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, bank, addrs[0])
        .await?;
    let b1 = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, bank, addrs[1])
        .await?;
    let b2 = hub
        .client
        .peek()
        .clone()
        .read_register(device.clone(), port, bank, addrs[2])
        .await?;
    Ok(u32::from_le_bytes([b0, b1, b2, 0]))
}

async fn write_u32_3byte(
    hub: &HubContext,
    device: &Device,
    port: u8,
    bank: u8,
    addrs: [u8; 3],
    val: u32,
) -> anyhow::Result<()> {
    let bytes = val.to_le_bytes();
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, bank, addrs[0], bytes[0])
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, bank, addrs[1], bytes[1])
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, bank, addrs[2], bytes[2])
        .await?;
    Ok(())
}

// --- PAJ Gain table ---

struct GainEntry {
    label: &'static str,
    b_global: u8,
    b_ggh: u8,
}

fn gain_index_from_reg(b_global: u8, mut b_ggh: u8) -> usize {
    if b_ggh > 0 {
        b_ggh -= 1;
    }
    (b_ggh as usize) * 16 + (b_global as usize)
}

static GAIN_TABLE: &[GainEntry] = &[
    // 1x multiplier (b_ggh=0)
    GainEntry {
        label: "1.0000",
        b_global: 0,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.0625",
        b_global: 1,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.1250",
        b_global: 2,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.1875",
        b_global: 3,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.2500",
        b_global: 4,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.3125",
        b_global: 5,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.3750",
        b_global: 6,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.4375",
        b_global: 7,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.5000",
        b_global: 8,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.5625",
        b_global: 9,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.6250",
        b_global: 10,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.6875",
        b_global: 11,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.7500",
        b_global: 12,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.8125",
        b_global: 13,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.8750",
        b_global: 14,
        b_ggh: 0,
    },
    GainEntry {
        label: "1.9375",
        b_global: 15,
        b_ggh: 0,
    },
    // 2x multiplier (b_ggh=2)
    GainEntry {
        label: "2.0000",
        b_global: 0,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.1250",
        b_global: 1,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.2500",
        b_global: 2,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.3750",
        b_global: 3,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.5000",
        b_global: 4,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.6250",
        b_global: 5,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.7500",
        b_global: 6,
        b_ggh: 2,
    },
    GainEntry {
        label: "2.8750",
        b_global: 7,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.0000",
        b_global: 8,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.1250",
        b_global: 9,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.2500",
        b_global: 10,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.3750",
        b_global: 11,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.5000",
        b_global: 12,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.6250",
        b_global: 13,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.7500",
        b_global: 14,
        b_ggh: 2,
    },
    GainEntry {
        label: "3.8750",
        b_global: 15,
        b_ggh: 2,
    },
    // 4x multiplier (b_ggh=3)
    GainEntry {
        label: "4.0000",
        b_global: 0,
        b_ggh: 3,
    },
    GainEntry {
        label: "4.2500",
        b_global: 1,
        b_ggh: 3,
    },
    GainEntry {
        label: "4.5000",
        b_global: 2,
        b_ggh: 3,
    },
    GainEntry {
        label: "4.7500",
        b_global: 3,
        b_ggh: 3,
    },
    GainEntry {
        label: "5.0000",
        b_global: 4,
        b_ggh: 3,
    },
    GainEntry {
        label: "5.2500",
        b_global: 5,
        b_ggh: 3,
    },
    GainEntry {
        label: "5.5000",
        b_global: 6,
        b_ggh: 3,
    },
    GainEntry {
        label: "5.7500",
        b_global: 7,
        b_ggh: 3,
    },
    GainEntry {
        label: "6.0000",
        b_global: 8,
        b_ggh: 3,
    },
    GainEntry {
        label: "6.2500",
        b_global: 9,
        b_ggh: 3,
    },
    GainEntry {
        label: "6.5000",
        b_global: 10,
        b_ggh: 3,
    },
    GainEntry {
        label: "6.7500",
        b_global: 11,
        b_ggh: 3,
    },
    GainEntry {
        label: "7.0000",
        b_global: 12,
        b_ggh: 3,
    },
    GainEntry {
        label: "7.2500",
        b_global: 13,
        b_ggh: 3,
    },
    GainEntry {
        label: "7.5000",
        b_global: 14,
        b_ggh: 3,
    },
    GainEntry {
        label: "7.7500",
        b_global: 15,
        b_ggh: 3,
    },
    GainEntry {
        label: "8.0000",
        b_global: 16,
        b_ggh: 3,
    },
];

// --- PAJ Sensor Panel ---

#[derive(Clone, PartialEq)]
struct PajState {
    brightness_threshold: String,
    noise_threshold: String,
    area_threshold_min: String,
    area_threshold_max: String,
    max_object_cnt: String,
    operation_mode: String,
    exposure_time: String,
    frame_period: String,
    frame_subtraction: String,
    gain_index: String,
    resolution_x: String,
    resolution_y: String,
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

async fn paj_load_from_device(
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

/// The fields that live in the device's general config, already validated.
#[derive(Clone, Copy, PartialEq)]
struct GeneralValues {
    impact_threshold: u8,
    suppress_ms: u8,
    shot_delay: Option<u16>,
}

fn general_validate(
    impact_threshold: &str,
    suppress_ms: &str,
    shot_delay: Option<&str>,
) -> Result<GeneralValues, Vec<FieldIssue>> {
    let mut issues = Vec::new();
    let mut take = |field, raw: &str, max| match parse_bounded(field, raw, max) {
        Ok(v) => Some(v),
        Err(e) => {
            issues.push(e);
            None
        }
    };

    let impact = take("Impact threshold", impact_threshold, u8::MAX as u32);
    let suppress = take("Suppress", suppress_ms, u8::MAX as u32);
    let delay = shot_delay.map(|d| take("Shot delay", d, u16::MAX as u32));

    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(GeneralValues {
        impact_threshold: impact.unwrap() as u8,
        suppress_ms: suppress.unwrap() as u8,
        shot_delay: delay.map(|d| d.unwrap() as u16),
    })
}

/// Write the general config. Callers validate first, so a typo in one field
/// cannot leave the device holding a half-applied set.
async fn write_general_config(
    hub: &HubContext,
    device: &Device,
    values: GeneralValues,
) -> anyhow::Result<()> {
    let mut client = hub.client.peek().clone();
    client
        .write_config(device.clone(), 0, values.impact_threshold)
        .await?;
    client
        .write_config(device.clone(), 1, values.suppress_ms)
        .await?;
    if let Some(delay) = values.shot_delay {
        client.set_shot_delay(device.clone(), delay).await?;
    }
    Ok(())
}

/// PAJ sensor settings after validation, in the units the registers expect.
#[derive(Clone, Copy, PartialEq)]
struct PajValues {
    brightness_threshold: u8,
    noise_threshold: u8,
    area_threshold_min: u8,
    area_threshold_max: u16,
    max_object_cnt: u8,
    operation_mode: u8,
    exposure_time: u16,
    frame_period: u32,
    frame_subtraction: u8,
    gain_index: usize,
    resolution_x: u16,
    resolution_y: u16,
}

fn paj_validate(state: &PajState) -> Result<PajValues, Vec<FieldIssue>> {
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

async fn paj_apply_to_device(
    hub: &HubContext,
    device: &Device,
    port: u8,
    values: &PajValues,
) -> anyhow::Result<()> {
    let gain = &GAIN_TABLE[values.gain_index];

    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x0C, 0x47, values.brightness_threshold)
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

fn paj_load_defaults(port: u8) -> PajState {
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

const INPUT_CLASS: &str = "bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg \
    focus:ring-blue-500 focus:border-blue-500 block w-full p-1.5 \
    dark:bg-gray-700 dark:border-gray-600 dark:text-white \
    dark:focus:ring-blue-500 dark:focus:border-blue-500";

const SELECT_CLASS: &str = "bg-gray-50 border border-gray-300 text-gray-900 text-sm rounded-lg \
    focus:ring-blue-500 focus:border-blue-500 block w-full p-1.5 \
    dark:bg-gray-700 dark:border-gray-600 dark:text-white \
    dark:focus:ring-blue-500 dark:focus:border-blue-500";

const LABEL_CLASS: &str = "text-sm text-gray-700 dark:text-gray-300 whitespace-nowrap";

const INVALID_INPUT_CLASS: &str = "bg-gray-50 border border-red-500 text-gray-900 text-sm rounded-lg \
    focus:ring-red-500 focus:border-red-500 block w-full p-1.5 \
    dark:bg-gray-700 dark:border-red-500 dark:text-white \
    dark:focus:ring-red-500 dark:focus:border-red-500";

// Disabled styling matters here: the write buttons go dead whenever a field is
// invalid, and a button that looks live but ignores clicks reads as a bug.
const ACTION_PRIMARY_CLASS: &str =
    "btn-primary text-xs px-2 py-1 disabled:opacity-50 disabled:cursor-not-allowed";

const ACTION_SECONDARY_CLASS: &str =
    "btn-secondary text-xs px-2 py-1 disabled:opacity-50 disabled:cursor-not-allowed";

/// A labelled numeric field with an optional unit hint and validation message.
///
/// `hint` carries the converted engineering value (ms, fps) so the raw register
/// numbers stop being the only thing on screen.
#[component]
fn NumberField(
    label: String,
    value: String,
    hint: Option<String>,
    issue: Option<String>,
    oninput: EventHandler<String>,
) -> Element {
    let invalid = issue.is_some();
    rsx! {
        label { class: LABEL_CLASS, "{label}" }
        div {
            class: "space-y-0.5",
            div {
                class: "flex items-center gap-2",
                input {
                    class: if invalid { INVALID_INPUT_CLASS } else { INPUT_CLASS },
                    r#type: "text",
                    inputmode: "numeric",
                    value: "{value}",
                    oninput: move |e| oninput.call(e.value()),
                }
                if let Some(hint) = hint {
                    span {
                        class: "text-xs text-gray-500 dark:text-gray-400 whitespace-nowrap",
                        "{hint}"
                    }
                }
            }
            if let Some(issue) = issue {
                p { class: "text-xs text-red-600 dark:text-red-400", "{issue}" }
            }
        }
    }
}

// Field names are shared between the validator and the panel so an issue can be
// routed back to the input that produced it.
const PAJ_BRIGHTNESS: &str = "Brightness threshold";
const PAJ_NOISE: &str = "Noise threshold";
const PAJ_AREA_MIN: &str = "Area threshold min";
const PAJ_AREA_MAX: &str = "Area threshold max";
const PAJ_MAX_OBJECTS: &str = "Max object count";
const PAJ_EXPOSURE: &str = "Exposure time";
const PAJ_FRAME_PERIOD: &str = "Frame period";
const PAJ_RESOLUTION_X: &str = "Resolution X";
const PAJ_RESOLUTION_Y: &str = "Resolution Y";

#[component]
fn PajSensorPanel(label: String, state: Signal<PajState>, show_issues: bool) -> Element {
    let issues = paj_validate(&state.read()).err().unwrap_or_default();
    let issue = |field: &str| {
        show_issues
            .then(|| issue_for(&issues, field).map(str::to_string))
            .flatten()
    };

    let exposure_hint = {
        let val: f64 = state.read().exposure_time.parse().unwrap_or(0.0);
        format!("{:.2} ms", val * 200.0 / 1e6)
    };
    let frame_hint = {
        let val: f64 = state.read().frame_period.parse().unwrap_or(0.0);
        if val > 0.0 {
            format!("{:.2} ms ({:.1} fps)", val / 1e4, 1e7 / val)
        } else {
            "---".into()
        }
    };

    rsx! {
        div {
            class: "space-y-2",
            h4 {
                class: "text-sm font-semibold text-gray-800 dark:text-gray-200",
                "{label}"
            }
            div {
                class: "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center",

                NumberField {
                    label: PAJ_BRIGHTNESS,
                    value: state.read().brightness_threshold.clone(),
                    issue: issue(PAJ_BRIGHTNESS),
                    oninput: move |v| state.write().brightness_threshold = v,
                }
                NumberField {
                    label: PAJ_NOISE,
                    value: state.read().noise_threshold.clone(),
                    issue: issue(PAJ_NOISE),
                    oninput: move |v| state.write().noise_threshold = v,
                }
                NumberField {
                    label: PAJ_AREA_MIN,
                    value: state.read().area_threshold_min.clone(),
                    hint: "px",
                    issue: issue(PAJ_AREA_MIN),
                    oninput: move |v| state.write().area_threshold_min = v,
                }
                NumberField {
                    label: PAJ_AREA_MAX,
                    value: state.read().area_threshold_max.clone(),
                    hint: "px",
                    issue: issue(PAJ_AREA_MAX),
                    oninput: move |v| state.write().area_threshold_max = v,
                }
                NumberField {
                    label: PAJ_MAX_OBJECTS,
                    value: state.read().max_object_cnt.clone(),
                    issue: issue(PAJ_MAX_OBJECTS),
                    oninput: move |v| state.write().max_object_cnt = v,
                }

                label { class: LABEL_CLASS, "Operation mode" }
                select {
                    class: SELECT_CLASS,
                    value: "{state.read().operation_mode}",
                    onchange: move |e| state.write().operation_mode = e.value(),
                    option { value: "0", "Normal" }
                    option { value: "1", "Tracking" }
                }

                NumberField {
                    label: PAJ_EXPOSURE,
                    value: state.read().exposure_time.clone(),
                    hint: exposure_hint,
                    issue: issue(PAJ_EXPOSURE),
                    oninput: move |v| state.write().exposure_time = v,
                }
                NumberField {
                    label: PAJ_FRAME_PERIOD,
                    value: state.read().frame_period.clone(),
                    hint: frame_hint,
                    issue: issue(PAJ_FRAME_PERIOD),
                    oninput: move |v| state.write().frame_period = v,
                }

                label { class: LABEL_CLASS, "Frame subtraction" }
                select {
                    class: SELECT_CLASS,
                    value: "{state.read().frame_subtraction}",
                    onchange: move |e| state.write().frame_subtraction = e.value(),
                    option { value: "0", "Off" }
                    option { value: "1", "On" }
                }

                label { class: LABEL_CLASS, "Gain" }
                select {
                    class: SELECT_CLASS,
                    value: "{state.read().gain_index}",
                    onchange: move |e| state.write().gain_index = e.value(),
                    for (i, entry) in GAIN_TABLE.iter().enumerate() {
                        option { value: "{i}", "{entry.label}" }
                    }
                }

                NumberField {
                    label: PAJ_RESOLUTION_X,
                    value: state.read().resolution_x.clone(),
                    issue: issue(PAJ_RESOLUTION_X),
                    oninput: move |v| state.write().resolution_x = v,
                }
                NumberField {
                    label: PAJ_RESOLUTION_Y,
                    value: state.read().resolution_y.clone(),
                    issue: issue(PAJ_RESOLUTION_Y),
                    oninput: move |v| state.write().resolution_y = v,
                }
            }
        }
    }
}

// --- PAG Sensor Panel ---

#[derive(Clone, PartialEq)]
struct PagState {
    fps: String,
    exposure_us: String,
    gain: String,
    light_threshold: String,
    area_lower: String,
    area_upper: String,
}

impl Default for PagState {
    fn default() -> Self {
        Self {
            fps: String::new(),
            exposure_us: String::new(),
            gain: String::new(),
            light_threshold: String::new(),
            area_lower: String::new(),
            area_upper: String::new(),
        }
    }
}

// PAG7665 register addresses (used for product_id 0x5211)
async fn pag_load_from_device(hub: &HubContext, device: &Device) -> anyhow::Result<PagState> {
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
    let exposure_us = (expo_raw & 0x7F) as u16 * 100;

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
struct PagValues {
    fps: u8,
    exposure_us: u16,
    gain: u8,
    light_threshold: u8,
    area_lower: u16,
    area_upper: u16,
}

// The exposure register holds 100us units in its low 7 bits, so 12700us is the
// ceiling and anything finer than 100us is truncated by the device.
const PAG_MAX_EXPOSURE_US: u32 = 12_700;

fn pag_validate(state: &PagState) -> Result<PagValues, Vec<FieldIssue>> {
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

async fn pag_apply_to_device(
    hub: &HubContext,
    device: &Device,
    values: &PagValues,
) -> anyhow::Result<()> {
    let port: u8 = 0;
    let led_always_on = true;
    let expo_reg = (values.exposure_us / 100) as u8 | ((led_always_on as u8) << 7);

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

fn pag_load_defaults() -> PagState {
    PagState {
        fps: "180".into(),
        exposure_us: "2000".into(),
        gain: "2".into(),
        light_threshold: "120".into(),
        area_lower: "10".into(),
        area_upper: "65535".into(),
    }
}

const PAG_FPS: &str = "Frame rate";
const PAG_EXPOSURE: &str = "Exposure";
const PAG_GAIN: &str = "Gain";
const PAG_LIGHT: &str = "Light threshold";
const PAG_AREA_MIN: &str = "Area threshold min";
const PAG_AREA_MAX: &str = "Area threshold max";

#[component]
fn PagSensorPanel(state: Signal<PagState>, show_issues: bool) -> Element {
    let issues = pag_validate(&state.read()).err().unwrap_or_default();
    let issue = |field: &str| {
        show_issues
            .then(|| issue_for(&issues, field).map(str::to_string))
            .flatten()
    };

    let exposure_hint = {
        let val: f64 = state.read().exposure_us.parse().unwrap_or(0.0);
        format!("{:.1} ms", val / 1000.0)
    };

    rsx! {
        div {
            class: "space-y-2",
            h4 {
                class: "text-sm font-semibold text-gray-800 dark:text-gray-200",
                "Sensor"
            }
            div {
                class: "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center",

                NumberField {
                    label: PAG_FPS,
                    value: state.read().fps.clone(),
                    hint: "fps",
                    issue: issue(PAG_FPS),
                    oninput: move |v| state.write().fps = v,
                }
                NumberField {
                    label: PAG_EXPOSURE,
                    value: state.read().exposure_us.clone(),
                    hint: "µs · {exposure_hint}",
                    issue: issue(PAG_EXPOSURE),
                    oninput: move |v| state.write().exposure_us = v,
                }
                NumberField {
                    label: PAG_GAIN,
                    value: state.read().gain.clone(),
                    issue: issue(PAG_GAIN),
                    oninput: move |v| state.write().gain = v,
                }
                NumberField {
                    label: PAG_LIGHT,
                    value: state.read().light_threshold.clone(),
                    issue: issue(PAG_LIGHT),
                    oninput: move |v| state.write().light_threshold = v,
                }
                NumberField {
                    label: PAG_AREA_MIN,
                    value: state.read().area_lower.clone(),
                    hint: "px",
                    issue: issue(PAG_AREA_MIN),
                    oninput: move |v| state.write().area_lower = v,
                }
                NumberField {
                    label: PAG_AREA_MAX,
                    value: state.read().area_upper.clone(),
                    hint: "px",
                    issue: issue(PAG_AREA_MAX),
                    oninput: move |v| state.write().area_upper = v,
                }
            }
        }
    }
}

// --- Transport Mode Panel ---

/// Shows a transport mode selector (BLE / USB) for ATS Lite / Lite1 devices.
/// Requires only CONTROL capability, so it is visible even when the device is
/// in BLE mode and the EVENTS bulk endpoint is not available.
#[component]
pub fn TransportModePanel(hub: Signal<HubContext>, device: Device) -> Element {
    let pid = device.product_id;
    // Only ATS Lite (0x5210) and Lite1 (0x5211) support transport mode switching.
    if pid != PID_ATS_LITE && pid != PID_ATS_PRO {
        return rsx! {};
    }
    if !device.capabilities.contains(DeviceCapabilities::CONTROL) {
        return rsx! {};
    }

    let mut status = use_signal(OpStatus::default);
    // user_pending_usb tracks the mode the user explicitly requested (None = no override).
    // It is cleared when the device prop reflects the new mode (i.e. after reboot).
    let mut user_pending_usb = use_signal(|| Option::<bool>::None);

    // Derive the displayed mode: prefer the user override while waiting for reboot,
    // otherwise use the actual device state from the prop (updates on every render).
    let device_is_usb = device.events_transport == odyssey_hub_common::device::EventsTransport::Wired;
    // Once the device reflects the user's requested mode, drop the override and clear the status.
    // We schedule this via spawn so we don't write to signals during the render phase.
    if let Some(u) = *user_pending_usb.read() {
        if u == device_is_usb {
            spawn(async move {
                user_pending_usb.set(None);
                status.set(OpStatus::Idle);
            });
        }
    }
    let pending_usb = user_pending_usb.read().unwrap_or(device_is_usb);

    let device_for_set = device.clone();
    let set_mode = std::sync::Arc::new(move |usb_mode: bool| {
        let device = device_for_set.clone();
        async move {
            status.set(OpStatus::Busy("Switching...".into()));
            let hub_ctx = hub.peek().clone();
            let mut client = hub_ctx.client.peek().clone();
            match client.set_transport_mode(device, usb_mode).await {
                Ok(confirmed_usb) => {
                    user_pending_usb.set(Some(confirmed_usb));
                    let mode_str = if confirmed_usb { "USB" } else { "Bluetooth" };
                    status.set(OpStatus::Busy(format!(
                        "Switching to {mode_str}. The device restarts and reconnects on its own."
                    )));
                }
                Err(e) => {
                    tracing::error!("set_transport_mode failed: {e:#}");
                    status.set(rpc_failure("switch the transport mode", &e));
                }
            }
        }
    });

    rsx! {
        details {
            class: "mt-2",
            summary {
                class: "text-sm font-medium text-gray-700 dark:text-gray-300 cursor-pointer \
                        hover:text-blue-600 dark:hover:text-blue-400 select-none",
                "Connection"
            }
            div {
                class: "mt-2 space-y-2 p-3 bg-gray-100 dark:bg-gray-700 rounded-lg",
                div {
                    class: "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center",
                    label { class: LABEL_CLASS, "Transport mode" }
                    div {
                        class: "flex gap-3",
                        button {
                            class: if !pending_usb {
                                "btn-primary text-xs px-2 py-1"
                            } else {
                                "btn-secondary text-xs px-2 py-1"
                            },
                            onclick: { let set_mode = set_mode.clone(); move |_| set_mode(false) },
                            "Bluetooth"
                        }
                        button {
                            class: if pending_usb {
                                "btn-primary text-xs px-2 py-1"
                            } else {
                                "btn-secondary text-xs px-2 py-1"
                            },
                            onclick: move |_| set_mode(true),
                            "USB"
                        }
                    }
                }
                p {
                    class: "text-xs text-gray-500 dark:text-gray-400",
                    if pending_usb {
                        "USB carries sensor data and tuning. Bluetooth leaves the device wireless but limits it to basic settings."
                    } else {
                        "Bluetooth keeps the device wireless. Switch to USB to read or change sensor tuning."
                    }
                }
                StatusLine { status: status() }
            }
        }
    }
}

/// What the settings load reads: the device as it is now and whether its sensors
/// can be tuned, which depends on the events link being up.
#[derive(Clone, PartialEq)]
struct LoadTarget {
    device: Device,
    can_tune_sensors: bool,
}

// --- Main SensorSettings Component ---

/// Summarise invalid input without repeating every message; the offending
/// fields are already marked in place.
fn invalid_status(issues: &[FieldIssue]) -> OpStatus {
    let fields: Vec<&str> = issues.iter().map(|i| i.field).collect();
    let noun = if fields.len() == 1 { "field" } else { "fields" };
    OpStatus::Failed {
        summary: format!("Check the highlighted {noun}: {}", fields.join(", ")),
        detail: None,
    }
}

#[component]
pub fn SensorSettings(hub: Signal<HubContext>, device: Device) -> Element {
    let pid = device.product_id;
    let is_paj = pid == PID_ATS_VM || pid == PID_ATS_LITE;
    let is_pag = pid == PID_ATS_PRO;
    let has_control = device.capabilities.contains(DeviceCapabilities::CONTROL);
    let has_events = device.capabilities.contains(DeviceCapabilities::EVENTS);
    // ReadConfig/WriteConfig/FlashSettings fall back to the control plane, so the
    // general settings work in BLE mode too. Register access needs the events
    // transport and the hub rejects it outright when that is missing, so the
    // sensor panels stay hidden instead of failing on every read.
    let should_render = (is_paj || is_pag) && (has_control || has_events);
    let can_tune_sensors = has_events;

    // All hooks must be called unconditionally before any early returns.
    let mut status = use_signal(OpStatus::default);
    let mut loading = use_signal(|| false);
    // Empty fields before the first read are "not loaded yet", not "invalid";
    // marking them red on first paint is noise the user cannot act on.
    let mut hydrated = use_signal(|| false);

    // General config state
    let mut impact_threshold = use_signal(String::new);
    let mut suppress_ms = use_signal(String::new);
    // Shot delay (stored on PC, not device)
    let mut shot_delay = use_signal(String::new);

    // PAJ state signals (NF and WF)
    let mut paj_nf_state = use_signal(PajState::default);
    let mut paj_wf_state = use_signal(PajState::default);
    // PAG state signal
    let mut pag_state = use_signal(PagState::default);

    // Tracks the open/closed state of the Device Settings section.
    let mut is_open = use_signal(|| false);

    // Coroutine: receives () each time settings should be loaded from device.
    // Triggered on section expand, the Reload button, and when the sensor panels
    // first become available.
    //
    // The closure runs once, so it reads the device and the sensor capability
    // through a signal rather than capturing them: both change while the
    // component stays mounted, since a unit whose Bluetooth events link attaches
    // after the first render only gains its sensor panels then.
    let mut load_target =
        use_signal(|| LoadTarget { device: device.clone(), can_tune_sensors });
    let load_co = use_coroutine(move |mut rx: UnboundedReceiver<()>| async move {
        while rx.next().await.is_some() {
            loading.set(true);
            status.set(OpStatus::Busy("Reading from device...".into()));
            let hub_ctx = hub.peek().clone();
            let load_target = load_target.peek().clone();
            let device = load_target.device.clone();
            // Keep the first failure. A later read succeeding does not make the
            // form trustworthy, and reporting "Loaded" over a failed read is how
            // stale values end up being flashed back onto the device.
            let mut failure: Option<OpStatus> = None;

            match hub_ctx.client.peek().clone().read_config(device.clone(), 0).await {
                Ok(v) => impact_threshold.set(v.to_string()),
                Err(e) => {
                    tracing::error!("reading impact threshold: {e:#}");
                    failure.get_or_insert(rpc_failure("read the impact threshold", &e));
                }
            }
            match hub_ctx.client.peek().clone().read_config(device.clone(), 1).await {
                Ok(v) => suppress_ms.set(v.to_string()),
                Err(e) => {
                    tracing::error!("reading suppress time: {e:#}");
                    failure.get_or_insert(rpc_failure("read the suppress time", &e));
                }
            }
            match hub_ctx.client.peek().clone().get_shot_delay(device.clone()).await {
                Ok(v) => shot_delay.set(v.to_string()),
                Err(e) => {
                    tracing::error!("reading shot delay: {e:#}");
                    failure.get_or_insert(rpc_failure("read the shot delay", &e));
                }
            }

            if load_target.can_tune_sensors {
                let result = if is_paj {
                    match (
                        paj_load_from_device(&hub_ctx, &device, 0).await,
                        paj_load_from_device(&hub_ctx, &device, 1).await,
                    ) {
                        (Ok(nf), Ok(wf)) => {
                            paj_nf_state.set(nf);
                            paj_wf_state.set(wf);
                            Ok(())
                        }
                        (Err(e), _) | (_, Err(e)) => Err(e),
                    }
                } else {
                    pag_load_from_device(&hub_ctx, &device)
                        .await
                        .map(|s| pag_state.set(s))
                };
                if let Err(e) = result {
                    tracing::error!("reading sensor settings: {e:#}");
                    failure.get_or_insert(rpc_failure("read the sensor settings", &e));
                }
            }

            hydrated.set(true);
            status.set(failure.unwrap_or(OpStatus::Done("Loaded from device".into())));
            loading.set(false);
        }
    });

    // Keep the load target current, and fill the sensor panels as soon as they
    // appear: they render empty until something loads them, and a form that was
    // loaded before the events link came up never asked for them.
    {
        let device = device.clone();
        use_effect(use_reactive!(|(device, can_tune_sensors)| {
            let panels_appeared = !load_target.peek().can_tune_sensors && can_tune_sensors;
            load_target.set(LoadTarget { device, can_tune_sensors });
            if panels_appeared && *is_open.peek() {
                load_co.send(());
            }
        }));
    }

    // Guards applied after all hooks have been called.
    if !should_render {
        return rsx! {};
    }

    let general_issues = general_validate(
        &impact_threshold.read(),
        &suppress_ms.read(),
        Some(&shot_delay.read()),
    )
    .err()
    .unwrap_or_default();
    let sensor_issue_count = if !can_tune_sensors {
        0
    } else if is_paj {
        paj_validate(&paj_nf_state.read()).err().map_or(0, |v| v.len())
            + paj_validate(&paj_wf_state.read()).err().map_or(0, |v| v.len())
    } else {
        pag_validate(&pag_state.read()).err().map_or(0, |v| v.len())
    };
    let has_invalid = !general_issues.is_empty() || sensor_issue_count > 0;
    let busy = *loading.read();
    // Writes stay blocked on invalid input either way; only the inline markers
    // wait for the first load.
    let show_issues = hydrated();
    let general_issue = |field: &str| {
        show_issues
            .then(|| issue_for(&general_issues, field).map(str::to_string))
            .flatten()
    };

    let reload = move |_| {
        load_co.send(());
    };

    // One code path for both write buttons: `persist` adds the flash step, so
    // the two cannot drift apart in what they send to the device.
    let device_for_write = device.clone();
    let write_all = std::sync::Arc::new(move |persist: bool| {
        let device = device_for_write.clone();
        async move {
            loading.set(true);
            status.set(OpStatus::Busy(
                if persist { "Saving to device..." } else { "Applying..." }.into(),
            ));
            let hub_ctx = hub.peek().clone();

            let general = match general_validate(
                &impact_threshold.peek(),
                &suppress_ms.peek(),
                Some(&shot_delay.peek()),
            ) {
                Ok(values) => values,
                Err(issues) => {
                    status.set(invalid_status(&issues));
                    loading.set(false);
                    return;
                }
            };
            if let Err(e) = write_general_config(&hub_ctx, &device, general).await {
                tracing::error!("writing general config: {e:#}");
                status.set(rpc_failure("write the general settings", &e));
                loading.set(false);
                return;
            }

            let sensors: Result<(), OpStatus> = async {
                if !can_tune_sensors {
                    return Ok(());
                }
                if is_paj {
                    for (port, state) in [(0u8, paj_nf_state), (1u8, paj_wf_state)] {
                        let values = paj_validate(&state.peek()).map_err(|i| invalid_status(&i))?;
                        paj_apply_to_device(&hub_ctx, &device, port, &values)
                            .await
                            .map_err(|e| {
                                tracing::error!("writing sensor settings: {e:#}");
                                rpc_failure("write the sensor settings", &e)
                            })?;
                    }
                } else {
                    let values = pag_validate(&pag_state.peek()).map_err(|i| invalid_status(&i))?;
                    pag_apply_to_device(&hub_ctx, &device, &values)
                        .await
                        .map_err(|e| {
                            tracing::error!("writing sensor settings: {e:#}");
                            rpc_failure("write the sensor settings", &e)
                        })?;
                }
                Ok(())
            }
            .await;
            if let Err(failed) = sensors {
                status.set(failed);
                loading.set(false);
                return;
            }

            // Flash last: it persists whatever the device currently holds, so it
            // must only run once every write above has succeeded.
            if persist {
                if let Err(e) = hub_ctx
                    .client
                    .peek()
                    .clone()
                    .flash_settings(device.clone())
                    .await
                {
                    tracing::error!("flashing settings: {e:#}");
                    status.set(rpc_failure("save the settings to the device", &e));
                    loading.set(false);
                    return;
                }
                status.set(OpStatus::Done("Saved to the device".into()));
            } else {
                status.set(OpStatus::Done(
                    "Applied for this session. Power cycling restores the saved values.".into(),
                ));
            }
            loading.set(false);
        }
    });

    let restore_defaults = move |_| {
        impact_threshold.set("25".into());
        suppress_ms.set("25".into());
        if can_tune_sensors {
            if is_paj {
                paj_nf_state.set(paj_load_defaults(0));
                paj_wf_state.set(paj_load_defaults(1));
            } else {
                pag_state.set(pag_load_defaults());
            }
        }
        status.set(OpStatus::Done(
            "Defaults filled in. Nothing reaches the device until you save.".into(),
        ));
    };

    rsx! {
        details {
            class: "mt-2",
            open: *is_open.read(),
            summary {
                class: "text-sm font-medium text-gray-700 dark:text-gray-300 cursor-pointer \
                        hover:text-blue-600 dark:hover:text-blue-400 select-none",
                onclick: move |_| {
                    let opening = !*is_open.peek();
                    is_open.set(opening);
                    if opening {
                        load_co.send(());
                    }
                },
                "Device Settings"
            }
            div {
                class: "mt-2 space-y-3 p-3 bg-gray-100 dark:bg-gray-700 rounded-lg",

                // Action buttons
                div {
                    class: "flex items-center gap-2 flex-wrap",
                    button {
                        class: ACTION_PRIMARY_CLASS,
                        disabled: busy || has_invalid,
                        title: "Write these values and keep them across power cycles",
                        onclick: { let write_all = write_all.clone(); move |_| write_all(true) },
                        "Save to device"
                    }
                    button {
                        class: ACTION_SECONDARY_CLASS,
                        disabled: busy || has_invalid,
                        title: "Write these values for this session only",
                        onclick: { let write_all = write_all.clone(); move |_| write_all(false) },
                        "Try without saving"
                    }
                    button {
                        class: ACTION_SECONDARY_CLASS,
                        disabled: busy,
                        title: "Discard edits and re-read the device",
                        onclick: reload,
                        "Reload"
                    }
                    button {
                        class: ACTION_SECONDARY_CLASS,
                        disabled: busy,
                        title: "Fill the form with factory values; nothing is written yet",
                        onclick: restore_defaults,
                        "Restore defaults"
                    }
                    StatusLine { status: status() }
                }

                // General settings
                div {
                    class: "space-y-2",
                    h4 {
                        class: "text-sm font-semibold text-gray-800 dark:text-gray-200",
                        "Impact detection"
                    }
                    div {
                        class: "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center",

                        NumberField {
                            label: "Impact threshold",
                            value: impact_threshold(),
                            issue: general_issue("Impact threshold"),
                            oninput: move |v| impact_threshold.set(v),
                        }
                        NumberField {
                            label: "Suppress",
                            value: suppress_ms(),
                            hint: "ms",
                            issue: general_issue("Suppress"),
                            oninput: move |v| suppress_ms.set(v),
                        }
                    }
                }

                // Shot delay lives on this computer, not on the device.
                div {
                    class: "space-y-2",
                    h4 {
                        class: "text-sm font-semibold text-gray-800 dark:text-gray-200",
                        "This computer"
                    }
                    div {
                        class: "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center",

                        NumberField {
                            label: "Shot delay",
                            value: shot_delay(),
                            hint: "ms",
                            issue: general_issue("Shot delay"),
                            oninput: move |v| shot_delay.set(v),
                        }
                    }
                    p {
                        class: "text-xs text-gray-500 dark:text-gray-400",
                        "Stored on this computer, not on the device."
                    }
                }

                // Sensor panels
                if can_tune_sensors {
                    if is_paj {
                        PajSensorPanel {
                            label: "Near Field",
                            state: paj_nf_state,
                            show_issues: show_issues,
                        }
                        PajSensorPanel {
                            label: "Wide Field",
                            state: paj_wf_state,
                            show_issues: show_issues,
                        }
                    }
                    if is_pag {
                        PagSensorPanel { state: pag_state, show_issues: show_issues }
                    }
                } else {
                    div {
                        class: "space-y-1 border-t border-gray-300 dark:border-gray-600 pt-2",
                        h4 {
                            class: "text-sm font-semibold text-gray-800 dark:text-gray-200",
                            "Sensor tuning"
                        }
                        p {
                            class: "text-xs text-gray-500 dark:text-gray-400",
                            "Available over USB only. Set Transport mode to USB in the \
                             Connection section, then reopen this panel."
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok<T>(r: Result<T, Vec<FieldIssue>>) -> T {
        match r { Ok(v) => v, Err(e) => panic!("unexpected issues: {:?}", e.iter().map(|i| i.field).collect::<Vec<_>>()) }
    }
    fn issues<T>(r: Result<T, Vec<FieldIssue>>) -> Vec<FieldIssue> {
        match r { Ok(_) => panic!("expected issues"), Err(e) => e }
    }
    fn msg(r: Result<u32, FieldIssue>) -> String {
        match r { Ok(v) => panic!("expected issue, got {v}"), Err(e) => e.message }
    }

    #[test]
    fn bounds_and_messages() {
        assert_eq!(msg(parse_bounded("f", "", 255)), "Required");
        assert_eq!(msg(parse_bounded("f", "abc", 255)), "Whole number 0-255");
        assert_eq!(msg(parse_bounded("f", "256", 255)), "Must be 0-255");
        assert!(matches!(parse_bounded("f", " 40 ", 255), Ok(40)));
    }

    #[test]
    fn paj_defaults_round_trip() {
        let v = ok(paj_validate(&paj_load_defaults(0)));
        assert_eq!(v.brightness_threshold, 110);
        assert_eq!(v.frame_period, 49780);
        assert_eq!(v.resolution_x, 4095);
        assert_eq!(v.gain_index, 16);
        let entry = &GAIN_TABLE[v.gain_index];
        assert_eq!(gain_index_from_reg(entry.b_global, entry.b_ggh), v.gain_index);

        let mut bad = paj_load_defaults(0);
        bad.brightness_threshold = "300".into();
        bad.resolution_y = "9000".into();
        let fields: Vec<_> = issues(paj_validate(&bad)).iter().map(|i| i.field).collect();
        assert_eq!(fields, vec![PAJ_BRIGHTNESS, PAJ_RESOLUTION_Y]);
    }

    #[test]
    fn pag_exposure_encoding() {
        let v = ok(pag_validate(&pag_load_defaults()));
        assert_eq!(v.exposure_us, 2000);
        assert_eq!((v.exposure_us / 100) as u8 | 0x80, 0x94);

        let mut bad = pag_load_defaults();
        bad.exposure_us = "13000".into();
        assert_eq!(issues(pag_validate(&bad))[0].field, PAG_EXPOSURE);
    }

    #[test]
    fn general_collects_every_issue() {
        let found = issues(general_validate("300", "", Some("x")));
        assert_eq!(found.len(), 3);
        match invalid_status(&found) {
            OpStatus::Failed { summary, detail } => {
                assert!(summary.contains("Impact threshold"), "{summary}");
                assert!(detail.is_none());
            }
            _ => panic!("expected failure"),
        }
    }

    #[test]
    fn transport_error_becomes_actionable() {
        let err = anyhow::anyhow!("events transport not connected");
        match rpc_failure("read the sensor settings", &err) {
            OpStatus::Failed { summary, detail } => {
                assert!(summary.contains("USB"), "{summary}");
                assert!(!summary.contains("status:"), "{summary}");
                assert_eq!(detail.unwrap_or_default(), "events transport not connected");
            }
            _ => panic!("expected failure"),
        }
    }
}
