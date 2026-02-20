use crate::hub::HubContext;
use dioxus::{logger::tracing, prelude::*};
use odyssey_hub_common::device::{Device, DeviceCapabilities};

// Product ID constants
const PID_ATS_VM: u16 = 0x520F;
const PID_ATS_LITE: u16 = 0x5210;
const PID_ATS_PRO: u16 = 0x5211;

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

async fn paj_apply_to_device(
    hub: &HubContext,
    device: &Device,
    port: u8,
    state: &PajState,
) -> anyhow::Result<()> {
    let gain_idx: usize = state.gain_index.parse().unwrap_or(0);
    let gain = &GAIN_TABLE[gain_idx.min(GAIN_TABLE.len() - 1)];

    hub.client
        .peek()
        .clone()
        .write_register(
            device.clone(),
            port,
            0x0C,
            0x47,
            state.brightness_threshold.parse().unwrap_or(0),
        )
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(
            device.clone(),
            port,
            0x00,
            0x0F,
            state.noise_threshold.parse().unwrap_or(0),
        )
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(
            device.clone(),
            port,
            0x0C,
            0x46,
            state.area_threshold_min.parse().unwrap_or(0),
        )
        .await?;
    write_u16_reg(
        hub,
        device,
        port,
        0x00,
        [0x0B, 0x0C],
        state.area_threshold_max.parse().unwrap_or(0),
    )
    .await?;
    hub.client
        .peek()
        .clone()
        .write_register(
            device.clone(),
            port,
            0x00,
            0x19,
            state.max_object_cnt.parse().unwrap_or(16),
        )
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(
            device.clone(),
            port,
            0x00,
            0x12,
            state.operation_mode.parse().unwrap_or(0),
        )
        .await?;
    write_u16_reg(
        hub,
        device,
        port,
        0x0C,
        [0x0F, 0x10],
        state.exposure_time.parse().unwrap_or(0),
    )
    .await?;
    write_u32_3byte(
        hub,
        device,
        port,
        0x0C,
        [0x07, 0x08, 0x09],
        state.frame_period.parse().unwrap_or(49780),
    )
    .await?;
    hub.client
        .peek()
        .clone()
        .write_register(
            device.clone(),
            port,
            0x00,
            0x28,
            state.frame_subtraction.parse().unwrap_or(0),
        )
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
    write_u16_reg(
        hub,
        device,
        port,
        0x0C,
        [0x60, 0x61],
        state.resolution_x.parse().unwrap_or(4095),
    )
    .await?;
    write_u16_reg(
        hub,
        device,
        port,
        0x0C,
        [0x62, 0x63],
        state.resolution_y.parse().unwrap_or(4095),
    )
    .await?;
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

#[component]
fn PajSensorPanel(
    hub: Signal<HubContext>,
    device: Device,
    port: u8,
    label: String,
    state: Signal<PajState>,
) -> Element {
    let exposure_ms = {
        let val: f64 = state.read().exposure_time.parse().unwrap_or(0.0);
        format!("{:.2} ms", val * 200.0 / 1e6)
    };
    let frame_info = {
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

                label { class: LABEL_CLASS, "Brightness threshold" }
                input {
                    class: INPUT_CLASS,
                    r#type: "text",
                    value: "{state.read().brightness_threshold}",
                    oninput: move |e| state.write().brightness_threshold = e.value(),
                }

                label { class: LABEL_CLASS, "Noise threshold" }
                input {
                    class: INPUT_CLASS,
                    r#type: "text",
                    value: "{state.read().noise_threshold}",
                    oninput: move |e| state.write().noise_threshold = e.value(),
                }

                label { class: LABEL_CLASS, "Area threshold min" }
                input {
                    class: INPUT_CLASS,
                    r#type: "text",
                    value: "{state.read().area_threshold_min}",
                    oninput: move |e| state.write().area_threshold_min = e.value(),
                }

                label { class: LABEL_CLASS, "Area threshold max" }
                input {
                    class: INPUT_CLASS,
                    r#type: "text",
                    value: "{state.read().area_threshold_max}",
                    oninput: move |e| state.write().area_threshold_max = e.value(),
                }

                label { class: LABEL_CLASS, "Max object count" }
                input {
                    class: INPUT_CLASS,
                    r#type: "text",
                    value: "{state.read().max_object_cnt}",
                    oninput: move |e| state.write().max_object_cnt = e.value(),
                }

                label { class: LABEL_CLASS, "Operation mode" }
                select {
                    class: SELECT_CLASS,
                    value: "{state.read().operation_mode}",
                    onchange: move |e| state.write().operation_mode = e.value(),
                    option { value: "0", "Normal" }
                    option { value: "1", "Tracking" }
                }

                label { class: LABEL_CLASS, "Exposure time" }
                div {
                    class: "flex items-center gap-2",
                    input {
                        class: INPUT_CLASS,
                        r#type: "text",
                        value: "{state.read().exposure_time}",
                        oninput: move |e| state.write().exposure_time = e.value(),
                    }
                    span { class: "text-xs text-gray-500 dark:text-gray-400 whitespace-nowrap", "{exposure_ms}" }
                }

                label { class: LABEL_CLASS, "Frame period" }
                div {
                    class: "flex items-center gap-2",
                    input {
                        class: INPUT_CLASS,
                        r#type: "text",
                        value: "{state.read().frame_period}",
                        oninput: move |e| state.write().frame_period = e.value(),
                    }
                    span { class: "text-xs text-gray-500 dark:text-gray-400 whitespace-nowrap", "{frame_info}" }
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

                label { class: LABEL_CLASS, "Resolution X" }
                input {
                    class: INPUT_CLASS,
                    r#type: "text",
                    value: "{state.read().resolution_x}",
                    oninput: move |e| state.write().resolution_x = e.value(),
                }

                label { class: LABEL_CLASS, "Resolution Y" }
                input {
                    class: INPUT_CLASS,
                    r#type: "text",
                    value: "{state.read().resolution_y}",
                    oninput: move |e| state.write().resolution_y = e.value(),
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

async fn pag_apply_to_device(
    hub: &HubContext,
    device: &Device,
    state: &PagState,
) -> anyhow::Result<()> {
    let port: u8 = 0;
    let fps: u8 = state.fps.parse().unwrap_or(180);
    let exposure_us: u16 = state.exposure_us.parse().unwrap_or(2000);
    let led_always_on = true;
    let expo_reg = (exposure_us / 100) as u8 | ((led_always_on as u8) << 7);
    let gain: u8 = state.gain.parse().unwrap_or(2);
    let light_threshold: u8 = state.light_threshold.parse().unwrap_or(120);
    let area_lower: u16 = state.area_lower.parse().unwrap_or(10);
    let area_upper: u16 = state.area_upper.parse().unwrap_or(65535);

    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x00, 0x13, fps)
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x00, 0x67, expo_reg)
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x00, 0x68, gain)
        .await?;
    hub.client
        .peek()
        .clone()
        .write_register(device.clone(), port, 0x00, 0x6D, light_threshold)
        .await?;
    write_u16_reg(hub, device, port, 0x00, [0x6E, 0x6F], area_lower).await?;
    write_u16_reg(hub, device, port, 0x00, [0x70, 0x71], area_upper).await?;
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

#[component]
fn PagSensorPanel(hub: Signal<HubContext>, device: Device, state: Signal<PagState>) -> Element {
    rsx! {
        div {
            class: "space-y-2",
            h4 {
                class: "text-sm font-semibold text-gray-800 dark:text-gray-200",
                "PAG Sensor"
            }
            div {
                class: "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center",

                label { class: LABEL_CLASS, "FPS" }
                input {
                    class: INPUT_CLASS,
                    r#type: "text",
                    value: "{state.read().fps}",
                    oninput: move |e| state.write().fps = e.value(),
                }

                label { class: LABEL_CLASS, "Exposure (us)" }
                input {
                    class: INPUT_CLASS,
                    r#type: "text",
                    value: "{state.read().exposure_us}",
                    oninput: move |e| state.write().exposure_us = e.value(),
                }

                label { class: LABEL_CLASS, "Gain" }
                input {
                    class: INPUT_CLASS,
                    r#type: "text",
                    value: "{state.read().gain}",
                    oninput: move |e| state.write().gain = e.value(),
                }

                label { class: LABEL_CLASS, "Light threshold" }
                input {
                    class: INPUT_CLASS,
                    r#type: "text",
                    value: "{state.read().light_threshold}",
                    oninput: move |e| state.write().light_threshold = e.value(),
                }

                label { class: LABEL_CLASS, "Area threshold min" }
                input {
                    class: INPUT_CLASS,
                    r#type: "text",
                    value: "{state.read().area_lower}",
                    oninput: move |e| state.write().area_lower = e.value(),
                }

                label { class: LABEL_CLASS, "Area threshold max" }
                input {
                    class: INPUT_CLASS,
                    r#type: "text",
                    value: "{state.read().area_upper}",
                    oninput: move |e| state.write().area_upper = e.value(),
                }
            }
        }
    }
}

// --- Main SensorSettings Component ---

#[component]
pub fn SensorSettings(hub: Signal<HubContext>, device: Device) -> Element {
    let pid = device.product_id;
    let is_paj = pid == PID_ATS_VM || pid == PID_ATS_LITE;
    let is_pag = pid == PID_ATS_PRO;

    if !is_paj && !is_pag {
        return rsx! {};
    }

    // Sensor settings require bulk endpoint access (EVENTS capability).
    // Devices in BLE transport mode with only CONTROL capability cannot
    // service bulk requests, so don't attempt to read/write config.
    if !device.capabilities.contains(DeviceCapabilities::EVENTS) {
        return rsx! {};
    }

    let mut status = use_signal(|| String::new());
    let mut loading = use_signal(|| false);

    // General config state
    let mut impact_threshold = use_signal(|| String::new());
    let mut suppress_ms = use_signal(|| String::new());
    // Shot delay (stored on PC, not device)
    let mut shot_delay = use_signal(|| String::new());

    // PAJ state signals (NF and WF)
    let mut paj_nf_state = use_signal(PajState::default);
    let mut paj_wf_state = use_signal(PajState::default);
    // PAG state signal
    let mut pag_state = use_signal(PagState::default);

    // Auto-load current values on mount
    {
        let device = device.clone();
        use_future(move || {
            let device = device.clone();
            async move {
                loading.set(true);
                status.set("Reading...".into());
                let hub_ctx = hub.peek().clone();
                // Load general config
                if let Ok(v) = hub_ctx
                    .client
                    .peek()
                    .clone()
                    .read_config(device.clone(), 0)
                    .await
                {
                    impact_threshold.set(v.to_string());
                }
                if let Ok(v) = hub_ctx
                    .client
                    .peek()
                    .clone()
                    .read_config(device.clone(), 1)
                    .await
                {
                    suppress_ms.set(v.to_string());
                }
                // Load shot delay
                if let Ok(v) = hub_ctx
                    .client
                    .peek()
                    .clone()
                    .get_shot_delay(device.clone())
                    .await
                {
                    shot_delay.set(v.to_string());
                }
                // Load sensor settings
                let result = if is_paj {
                    let nf = paj_load_from_device(&hub_ctx, &device, 0).await;
                    let wf = paj_load_from_device(&hub_ctx, &device, 1).await;
                    match (nf, wf) {
                        (Ok(nf_s), Ok(wf_s)) => {
                            paj_nf_state.set(nf_s);
                            paj_wf_state.set(wf_s);
                            Ok(())
                        }
                        (Err(e), _) | (_, Err(e)) => Err(e),
                    }
                } else {
                    pag_load_from_device(&hub_ctx, &device)
                        .await
                        .map(|s| pag_state.set(s))
                };
                match result {
                    Ok(()) => status.set(String::new()),
                    Err(e) => {
                        tracing::error!("Failed to load settings: {e}");
                        status.set(format!("Error: {e}"));
                    }
                }
                loading.set(false);
            }
        });
    }

    let device_for_reload = device.clone();
    let device_for_apply = device.clone();
    let device_for_save = device.clone();

    let reload = move |_| {
        let device = device_for_reload.clone();
        async move {
            loading.set(true);
            status.set("Reading...".into());
            let hub_ctx = hub.peek().clone();
            // Reload general config
            if let Ok(v) = hub_ctx
                .client
                .peek()
                .clone()
                .read_config(device.clone(), 0)
                .await
            {
                impact_threshold.set(v.to_string());
            }
            if let Ok(v) = hub_ctx
                .client
                .peek()
                .clone()
                .read_config(device.clone(), 1)
                .await
            {
                suppress_ms.set(v.to_string());
            }
            // Reload shot delay
            if let Ok(v) = hub_ctx
                .client
                .peek()
                .clone()
                .get_shot_delay(device.clone())
                .await
            {
                shot_delay.set(v.to_string());
            }
            // Reload sensor settings
            let result = if is_paj {
                let nf = paj_load_from_device(&hub_ctx, &device, 0).await;
                let wf = paj_load_from_device(&hub_ctx, &device, 1).await;
                match (nf, wf) {
                    (Ok(nf_s), Ok(wf_s)) => {
                        paj_nf_state.set(nf_s);
                        paj_wf_state.set(wf_s);
                        Ok(())
                    }
                    (Err(e), _) | (_, Err(e)) => Err(e),
                }
            } else {
                pag_load_from_device(&hub_ctx, &device)
                    .await
                    .map(|s| pag_state.set(s))
            };
            match result {
                Ok(()) => status.set("Loaded".into()),
                Err(e) => {
                    tracing::error!("Failed to load sensor settings: {e}");
                    status.set(format!("Error: {e}"));
                }
            }
            loading.set(false);
        }
    };

    let apply = move |_| {
        let device = device_for_apply.clone();
        async move {
            loading.set(true);
            status.set("Applying...".into());
            let hub_ctx = hub.peek().clone();
            // Apply general config
            if let Ok(v) = impact_threshold.peek().parse::<u8>() {
                let _ = hub_ctx
                    .client
                    .peek()
                    .clone()
                    .write_config(device.clone(), 0, v)
                    .await;
            }
            if let Ok(v) = suppress_ms.peek().parse::<u8>() {
                let _ = hub_ctx
                    .client
                    .peek()
                    .clone()
                    .write_config(device.clone(), 1, v)
                    .await;
            }
            // Apply shot delay
            if let Ok(v) = shot_delay.peek().parse::<u16>() {
                let _ = hub_ctx
                    .client
                    .peek()
                    .clone()
                    .set_shot_delay(device.clone(), v)
                    .await;
            }
            // Apply sensor settings
            let result = if is_paj {
                let nf = paj_apply_to_device(&hub_ctx, &device, 0, &paj_nf_state.peek()).await;
                let wf = paj_apply_to_device(&hub_ctx, &device, 1, &paj_wf_state.peek()).await;
                nf.and(wf)
            } else {
                pag_apply_to_device(&hub_ctx, &device, &pag_state.peek()).await
            };
            match result {
                Ok(()) => status.set("Applied".into()),
                Err(e) => {
                    tracing::error!("Failed to apply settings: {e}");
                    status.set(format!("Error: {e}"));
                }
            }
            loading.set(false);
        }
    };

    let save = move |_| {
        let device = device_for_save.clone();
        async move {
            loading.set(true);
            status.set("Applying & saving...".into());
            let hub_ctx = hub.peek().clone();
            // Apply general config before saving
            if let Ok(v) = impact_threshold.peek().parse::<u8>() {
                let _ = hub_ctx
                    .client
                    .peek()
                    .clone()
                    .write_config(device.clone(), 0, v)
                    .await;
            }
            if let Ok(v) = suppress_ms.peek().parse::<u8>() {
                let _ = hub_ctx
                    .client
                    .peek()
                    .clone()
                    .write_config(device.clone(), 1, v)
                    .await;
            }
            // Apply sensor settings before saving
            let apply_result = if is_paj {
                let nf = paj_apply_to_device(&hub_ctx, &device, 0, &paj_nf_state.peek()).await;
                let wf = paj_apply_to_device(&hub_ctx, &device, 1, &paj_wf_state.peek()).await;
                nf.and(wf)
            } else {
                pag_apply_to_device(&hub_ctx, &device, &pag_state.peek()).await
            };
            if let Err(e) = apply_result {
                tracing::error!("Failed to apply before save: {e}");
                status.set(format!("Error: {e}"));
                loading.set(false);
                return;
            }
            match hub_ctx
                .client
                .peek()
                .clone()
                .flash_settings(device.clone())
                .await
            {
                Ok(()) => status.set("Saved".into()),
                Err(e) => {
                    tracing::error!("Failed to flash settings: {e}");
                    status.set(format!("Save error: {e}"));
                }
            }
            loading.set(false);
        }
    };

    let load_defaults = move |_| {
        impact_threshold.set("100".into());
        suppress_ms.set("100".into());
        if is_paj {
            paj_nf_state.set(paj_load_defaults(0));
            paj_wf_state.set(paj_load_defaults(1));
        } else {
            pag_state.set(pag_load_defaults());
        }
        status.set("Defaults loaded".into());
    };

    rsx! {
        details {
            class: "mt-2",
            summary {
                class: "text-sm font-medium text-gray-700 dark:text-gray-300 cursor-pointer \
                        hover:text-blue-600 dark:hover:text-blue-400 select-none",
                "Device Settings"
            }
            div {
                class: "mt-2 space-y-3 p-3 bg-gray-100 dark:bg-gray-700 rounded-lg",

                // Action buttons
                div {
                    class: "flex items-center gap-2 flex-wrap",
                    button {
                        class: "btn-secondary",
                        disabled: *loading.read(),
                        onclick: reload,
                        "Reload"
                    }
                    button {
                        class: "btn-secondary",
                        disabled: *loading.read(),
                        onclick: apply,
                        "Apply"
                    }
                    button {
                        class: "btn-secondary",
                        disabled: *loading.read(),
                        onclick: save,
                        "Save"
                    }
                    button {
                        class: "btn-secondary",
                        disabled: *loading.read(),
                        onclick: load_defaults,
                        "Load Defaults"
                    }
                    if !status.read().is_empty() {
                        span {
                            class: "text-xs text-gray-500 dark:text-gray-400 italic",
                            "{status}"
                        }
                    }
                }

                // General settings
                div {
                    class: "space-y-2",
                    h4 {
                        class: "text-sm font-semibold text-gray-800 dark:text-gray-200",
                        "General"
                    }
                    div {
                        class: "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center",

                        label { class: LABEL_CLASS, "Impact threshold" }
                        input {
                            class: INPUT_CLASS,
                            r#type: "text",
                            value: "{impact_threshold}",
                            oninput: move |e| impact_threshold.set(e.value()),
                        }

                        label { class: LABEL_CLASS, "Suppress (ms)" }
                        input {
                            class: INPUT_CLASS,
                            r#type: "text",
                            value: "{suppress_ms}",
                            oninput: move |e| suppress_ms.set(e.value()),
                        }
                    }
                }

                // System config (stored on PC)
                div {
                    class: "space-y-2",
                    h4 {
                        class: "text-sm font-semibold text-gray-800 dark:text-gray-200",
                        "System"
                    }
                    div {
                        class: "grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 items-center",

                        label { class: LABEL_CLASS, "Shot delay" }
                        input {
                            class: INPUT_CLASS,
                            r#type: "text",
                            value: "{shot_delay}",
                            oninput: move |e| shot_delay.set(e.value()),
                        }
                    }
                }

                // Sensor panels
                if is_paj {
                    PajSensorPanel {
                        hub: hub,
                        device: device.clone(),
                        port: 0,
                        label: "Near Field".to_string(),
                        state: paj_nf_state,
                    }
                    PajSensorPanel {
                        hub: hub,
                        device: device.clone(),
                        port: 1,
                        label: "Wide Field".to_string(),
                        state: paj_wf_state,
                    }
                }
                if is_pag {
                    PagSensorPanel {
                        hub: hub,
                        device: device.clone(),
                        state: pag_state,
                    }
                }
            }
        }
    }
}
