use crate::hub::HubContext;
use dioxus::{logger::tracing, prelude::*};
use odyssey_hub_client::client::DongleInfo;
use odyssey_hub_common::device::Device;

fn format_uuid(uuid: &[u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        uuid[0], uuid[1], uuid[2], uuid[3], uuid[4], uuid[5]
    )
}

fn semver_at_least(version: [u16; 3], required: [u16; 3]) -> bool {
    version[0] > required[0]
        || (version[0] == required[0] && version[1] > required[1])
        || (version[0] == required[0] && version[1] == required[1] && version[2] >= required[2])
}

fn device_name(device: &Device) -> String {
    match device.product_id {
        0x520F => "ATS VM".into(),
        0x5210 => "ATS Lite".into(),
        0x5211 => "ATS Lite1".into(),
        _ => "Unknown".into(),
    }
}

// -------------------- Dongle row --------------------

#[component]
fn DongleRow(hub: Signal<HubContext>, dongle: DongleInfo) -> Element {
    let mut status = use_signal(|| String::new());
    let mut pairing = use_signal(|| false);

    let id_for_pair = dongle.id.clone();
    let id_for_cancel = dongle.id.clone();
    let id_for_clear = dongle.id.clone();

    let on_pair = move |_| {
        let id = id_for_pair.clone();
        async move {
            pairing.set(true);
            status.set("Pairing...".into());
            let mut client = hub.peek().client.peek().clone();
            match client.start_dongle_pairing(id, 120_000).await {
                Ok(()) => {}
                Err(e) => {
                    tracing::error!("Failed to start dongle pairing: {e}");
                    status.set(format!("Error: {e}"));
                    pairing.set(false);
                }
            }
        }
    };

    let on_cancel = move |_| {
        let id = id_for_cancel.clone();
        async move {
            let mut client = hub.peek().client.peek().clone();
            match client.cancel_dongle_pairing(id).await {
                Ok(()) => {
                    status.set("Cancelled".into());
                    pairing.set(false);
                }
                Err(e) => {
                    tracing::error!("Failed to cancel dongle pairing: {e}");
                    status.set(format!("Error: {e}"));
                }
            }
        }
    };

    let on_clear = move |_| {
        let id = id_for_clear.clone();
        async move {
            let mut client = hub.peek().client.peek().clone();
            // Cancel pairing first if in progress
            if *pairing.peek() {
                let _ = client.cancel_dongle_pairing(id.clone()).await;
                pairing.set(false);
            }
            match client.clear_dongle_bonds(id).await {
                Ok(()) => {
                    status.set("Bonds cleared".into());
                }
                Err(e) => {
                    tracing::error!("Failed to clear dongle bonds: {e}");
                    status.set(format!("Error: {e}"));
                }
            }
        }
    };

    // Listen for dongle pairing result events
    {
        let dongle_id = dongle.id.clone();
        let latest_event = hub.peek().latest_event.clone();
        use_effect(move || {
            if let Some(odyssey_hub_common::events::Event::DonglePairingResult {
                dongle_id: event_id,
                success,
                paired_address,
                error,
            }) = &*latest_event.read()
            {
                if *event_id == dongle_id {
                    pairing.set(false);
                    if *success {
                        status.set(format!("Paired with {}", format_uuid(paired_address)));
                    } else {
                        status.set(error.clone());
                    }
                }
            }
        });
    }

    let fw = &dongle.firmware_version;
    let fw_str = if *fw == [0, 0, 0] {
        String::new()
    } else {
        format!(" (fw {}.{}.{})", fw[0], fw[1], fw[2])
    };
    let show_bonds = semver_at_least(dongle.control_protocol_version, [0, 1, 1]);
    let listed_addrs = if show_bonds {
        &dongle.bonded_devices
    } else {
        &dongle.connected_devices
    };
    let list_label = if show_bonds { "Bonds" } else { "Connected" };

    let devices = (hub.peek().devices)();

    rsx! {
        div {
            class: "p-3 bg-gray-50 dark:bg-gray-600 rounded-lg space-y-2",
            div {
                class: "flex items-center justify-between",
                div {
                    class: "flex items-center gap-2",
                    span {
                        class: "text-sm font-medium text-gray-700 dark:text-gray-200",
                        "Dongle"
                    }
                    span {
                        class: "text-xs font-mono text-gray-500 dark:text-gray-400",
                        "{dongle.id}{fw_str}"
                    }
                    if !status.read().is_empty() {
                        span {
                            class: "text-xs text-gray-500 dark:text-gray-400 italic",
                            "{status}"
                        }
                    }
                }
                div {
                    class: "flex items-center gap-1",
                    if *pairing.read() {
                        button {
                            class: "btn-secondary text-xs",
                            onclick: on_cancel,
                            "Cancel"
                        }
                    } else {
                        button {
                            class: "btn-secondary text-xs",
                            onclick: on_pair,
                            "Pair"
                        }
                    }
                    button {
                        class: "btn-secondary text-xs",
                        onclick: on_clear,
                        "Clear Bonds"
                    }
                }
            }
            if !listed_addrs.is_empty() {
                div {
                    class: "ml-4 space-y-1",
                    div {
                        class: "text-[11px] uppercase tracking-wide text-gray-500 dark:text-gray-400",
                        "{list_label}"
                    }
                    for addr in listed_addrs {
                        {
                            let is_connected = dongle.connected_devices.iter().any(|a| a == addr);
                            let dot_class = if is_connected {
                                "inline-block w-2 h-2 rounded-full bg-green-500"
                            } else {
                                "inline-block w-2 h-2 rounded-full bg-red-500"
                            };
                            let name = devices.iter()
                                .find(|(_, d)| d.uuid == *addr)
                                .map(|(_, d)| device_name(d));
                            let addr_str = format_uuid(addr);
                            rsx! {
                                div {
                                    class: "flex items-center gap-2",
                                    span {
                                        class: "{dot_class}",
                                    }
                                    span {
                                        class: "text-xs font-mono text-gray-600 dark:text-gray-300",
                                        "{addr_str}"
                                    }
                                    if let Some(name) = name {
                                        span {
                                            class: "text-xs text-gray-500 dark:text-gray-400",
                                            "({name})"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// -------------------- ATS device row --------------------

#[component]
fn DeviceRow(hub: Signal<HubContext>, device: Device) -> Element {
    let mut status = use_signal(|| String::new());
    let mut pairing = use_signal(|| false);

    let device_for_pair = device.clone();
    let device_for_cancel = device.clone();
    let device_for_clear = device.clone();

    let on_pair = move |_| {
        let device = device_for_pair.clone();
        async move {
            pairing.set(true);
            status.set("Pairing...".into());
            let mut client = hub.peek().client.peek().clone();
            match client.start_pairing(device, 120_000).await {
                Ok(()) => {}
                Err(e) => {
                    tracing::error!("Failed to start pairing: {e}");
                    status.set(format!("Error: {e}"));
                    pairing.set(false);
                }
            }
        }
    };

    let on_cancel = move |_| {
        let device = device_for_cancel.clone();
        async move {
            let mut client = hub.peek().client.peek().clone();
            match client.cancel_pairing(device).await {
                Ok(()) => {
                    status.set("Cancelled".into());
                    pairing.set(false);
                }
                Err(e) => {
                    tracing::error!("Failed to cancel pairing: {e}");
                    status.set(format!("Error: {e}"));
                }
            }
        }
    };

    let on_clear = move |_| {
        let device = device_for_clear.clone();
        async move {
            let mut client = hub.peek().client.peek().clone();
            // Cancel pairing first if in progress
            if *pairing.peek() {
                let _ = client.cancel_pairing(device.clone()).await;
                pairing.set(false);
            }
            match client.clear_bond(device).await {
                Ok(()) => {
                    status.set("Bond cleared".into());
                }
                Err(e) => {
                    tracing::error!("Failed to clear bond: {e}");
                    status.set(format!("Error: {e}"));
                }
            }
        }
    };

    // Listen for pairing result events for this device
    {
        let device_uuid = device.uuid;
        let latest_event = hub.peek().latest_event.clone();
        use_effect(move || {
            if let Some(odyssey_hub_common::events::Event::DeviceEvent(
                odyssey_hub_common::events::DeviceEvent(
                    dev,
                    odyssey_hub_common::events::DeviceEventKind::PairingResult {
                        success,
                        paired_address,
                        error,
                    },
                ),
            )) = &*latest_event.read()
            {
                if dev.uuid == device_uuid {
                    pairing.set(false);
                    if *success {
                        status.set(format!("Paired with {}", format_uuid(paired_address)));
                    } else {
                        status.set(error.clone());
                    }
                }
            }
        });
    }

    rsx! {
        div {
            class: "flex items-center justify-between p-3 bg-gray-50 dark:bg-gray-600 rounded-lg",
            div {
                class: "flex items-center gap-2",
                span {
                    class: "text-sm font-medium text-gray-700 dark:text-gray-200",
                    "{device_name(&device)}"
                }
                span {
                    class: "text-xs font-mono text-gray-500 dark:text-gray-400",
                    "{format_uuid(&device.uuid)}"
                }
                if !status.read().is_empty() {
                    span {
                        class: "text-xs text-gray-500 dark:text-gray-400 italic",
                        "{status}"
                    }
                }
            }
            div {
                class: "flex items-center gap-1",
                if *pairing.read() {
                    button {
                        class: "btn-secondary text-xs",
                        onclick: on_cancel,
                        "Cancel"
                    }
                } else {
                    button {
                        class: "btn-secondary text-xs",
                        onclick: on_pair,
                        "Pair"
                    }
                }
                button {
                    class: "btn-secondary text-xs",
                    onclick: on_clear,
                    "Clear Bond"
                }
            }
        }
    }
}

// -------------------- Page --------------------

#[component]
pub fn Pairing() -> Element {
    let hub = use_context::<Signal<HubContext>>();
    let devices = (hub().devices)();
    let dongles = (hub().dongles)();

    let ats_devices: Vec<Device> = devices.iter().map(|(_slot, d)| d.clone()).collect();

    rsx! {
        div {
            class: "min-h-screen flex flex-col items-center justify-center bg-gray-50 dark:bg-gray-800",
            div {
                class: "w-full max-w-md flex flex-col items-stretch gap-4",
                h1 {
                    class: "text-center text-2xl font-bold text-gray-900 dark:text-white",
                    "Pairing"
                }

                if dongles.is_empty() && ats_devices.is_empty() {
                    p {
                        class: "text-center text-gray-500 dark:text-gray-400",
                        "No devices connected"
                    }
                }

                if !dongles.is_empty() {
                    div {
                        class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow",
                        h2 {
                            class: "text-lg font-semibold text-gray-900 dark:text-white mb-3",
                            "Dongles"
                        }
                        div {
                            class: "space-y-2",
                            for dongle in dongles {
                                DongleRow {
                                    key: "{dongle.id}",
                                    hub: hub,
                                    dongle: dongle,
                                }
                            }
                        }
                    }
                }

                if !ats_devices.is_empty() {
                    div {
                        class: "bg-white dark:bg-gray-700 rounded-lg p-4 shadow",
                        h2 {
                            class: "text-lg font-semibold text-gray-900 dark:text-white mb-3",
                            "Devices"
                        }
                        div {
                            class: "space-y-2",
                            for ats_dev in ats_devices {
                                DeviceRow {
                                    key: "{ats_dev.uuid:02x?}",
                                    hub: hub,
                                    device: ats_dev,
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
