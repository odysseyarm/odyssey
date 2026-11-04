//! The shot delay stepper and its Set, Reset and Save buttons.

use dioxus::logger::tracing;
use dioxus::prelude::*;
use odyssey_hub_common::device::Device;

use super::device_signals::{DeviceSignals, MAX_SHOT_DELAY_MS};
use crate::hub::HubContext;

const BUTTON: &str = "btn-secondary";

const STEP_BUTTON_START: &str = "bg-gray-100 dark:bg-gray-700 dark:hover:bg-gray-600 dark:border-gray-600 \
                                hover:bg-gray-200 border border-gray-300 rounded-s-lg p-2 h-10 focus:ring-gray-100 \
                                dark:focus:ring-gray-700 focus:ring-2 focus:outline-none";
const STEP_BUTTON_END: &str = "bg-gray-100 dark:bg-gray-700 dark:hover:bg-gray-600 dark:border-gray-600 \
                                hover:bg-gray-200 border border-gray-300 rounded-e-lg p-2 h-10 focus:ring-gray-100 \
                                dark:focus:ring-gray-700 focus:ring-2 focus:outline-none";

/// The shot delay a typed value stands for: out-of-range numbers stick to the
/// nearest end of the range.
fn clamp_shot_delay(typed: i32) -> u16 {
    u16::try_from(typed.clamp(0, i32::from(MAX_SHOT_DELAY_MS))).unwrap_or(MAX_SHOT_DELAY_MS)
}

#[component]
pub(super) fn ShotDelayControl(
    hub: Signal<HubContext>,
    slot: usize,
    device: Device,
    signals: DeviceSignals,
) -> Element {
    let pending = use_signal(|| false);
    rsx! {
        li {
            class: "flex items-center gap-2",

            label {
                r#for: "shot-delay-input-{slot}",
                class: "text-sm text-gray-900 dark:text-white",
                "Shot Delay (ms):"
            }

            div {
                class: "relative flex items-center max-w-[6rem]",

                button {
                    id: "decrement-button",
                    class: STEP_BUTTON_START,
                    disabled: pending(),
                    onclick: move |_| {
                        let delay_ms = signals.shot_delay_ms();
                        if delay_ms > 0 {
                            signals.set_shot_delay_ms(delay_ms - 1);
                        }
                    },
                    svg {
                        class: "w-3 h-3 text-gray-900 dark:text-white",
                        xmlns: "http://www.w3.org/2000/svg",
                        fill: "none",
                        view_box: "0 0 18 2",
                        path {
                            d: "M1 1h16",
                            stroke: "currentColor",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            stroke_width: "2",
                        }
                    }
                }

                input {
                    r#type: "text",
                    inputmode: "numeric",
                    pattern: "[0-9]*",
                    id: "shot-delay-input-{slot}",
                    disabled: pending(),
                    value: "{signals.shot_delay}",
                    class: "bg-gray-50 border-x-0 border-gray-300 h-10 text-center text-gray-900 text-sm \
                            focus:ring-blue-500 focus:border-blue-500 block w-full py-1.5 \
                            dark:bg-gray-700 dark:border-gray-600 dark:placeholder-gray-400 \
                            dark:text-white dark:focus:ring-blue-500 dark:focus:border-blue-500",
                    oninput: move |e| {
                        if let Ok(typed) = e.value().parse::<i32>() {
                            signals.set_shot_delay_ms(clamp_shot_delay(typed));
                        }
                    }
                }

                button {
                    id: "increment-button",
                    class: STEP_BUTTON_END,
                    disabled: pending(),
                    onclick: move |_| {
                        let delay_ms = signals.shot_delay_ms();
                        if delay_ms < MAX_SHOT_DELAY_MS {
                            signals.set_shot_delay_ms(delay_ms + 1);
                        }
                    },
                    svg {
                        class: "w-3 h-3 text-gray-900 dark:text-white",
                        xmlns: "http://www.w3.org/2000/svg",
                        fill: "none",
                        view_box: "0 0 18 18",
                        path {
                            d: "M9 1v16M1 9h16",
                            stroke: "currentColor",
                            stroke_linecap: "round",
                            stroke_linejoin: "round",
                            stroke_width: "2",
                        }
                    }
                }
            }

            // Set button
            button {
                class: BUTTON,
                disabled: pending(),
                onclick: {
                    let device = device.clone();
                    move |_| run_command(pending, set_shot_delay(hub, device.clone(), signals.shot_delay_ms()))
                },
                "Set"
            }

            // Reset button
            button {
                class: BUTTON,
                disabled: pending(),
                onclick: {
                    let device = device.clone();
                    move |_| {
                        let revision = signals.shot_delay_revision();
                        run_command(pending, reset_shot_delay(hub, device.clone(), signals, revision));
                    }
                },
                "Reset"
            }

            // Save button (sets before saving)
            button {
                class: BUTTON,
                disabled: pending(),
                onclick: move |_| run_command(pending, set_and_save_shot_delay(hub, device.clone(), signals.shot_delay_ms())),
                "Save"
            }
        }
    }
}

/// Set-and-save is one device operation: another Set must not replace its
/// value between the two RPCs. Reserve the controls before spawning.
fn run_command(mut pending: Signal<bool>, command: impl Future<Output = ()> + 'static) {
    if *pending.peek() {
        return;
    }
    pending.set(true);
    spawn(async move {
        command.await;
        pending.set(false);
    });
}

async fn set_shot_delay(hub: Signal<HubContext>, device: Device, delay_ms: u16) {
    let mut client = hub.peek().client.peek().clone();
    match client.set_shot_delay(device.clone(), delay_ms).await {
        Ok(()) => tracing::info!("Set shot delay {}ms for {:x?}", delay_ms, device.uuid),
        Err(e) => tracing::error!("Failed to set shot delay {:x?}: {}", device.uuid, e),
    }
}

async fn reset_shot_delay(
    hub: Signal<HubContext>,
    device: Device,
    signals: DeviceSignals,
    revision: u64,
) {
    let mut client = hub.peek().client.peek().clone();
    match client.reset_shot_delay(device.clone()).await {
        Ok(delay_ms) => {
            tracing::info!("Reset shot delay for {:x?}", device.uuid);
            if signals.shot_delay_revision() == revision {
                signals.set_shot_delay_ms(delay_ms);
            }
        }
        Err(e) => tracing::error!("Failed to reset shot delay {:x?}: {}", device.uuid, e),
    }
}

async fn set_and_save_shot_delay(hub: Signal<HubContext>, device: Device, delay_ms: u16) {
    let mut client = hub.peek().client.peek().clone();
    if let Err(e) = client.set_shot_delay(device.clone(), delay_ms).await {
        tracing::error!("Failed to set shot delay {:x?}: {}", device.uuid, e);
        return;
    }
    tracing::info!("Set shot delay {}ms for {:x?}", delay_ms, device.uuid);
    match client.save_shot_delay(device.clone()).await {
        Ok(()) => tracing::info!("Saved shot delay for {:x?}", device.uuid),
        Err(e) => tracing::error!("Failed to save shot delay {:x?}: {}", device.uuid, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_delays_stick_to_the_range() {
        assert_eq!(clamp_shot_delay(-5), 0);
        assert_eq!(clamp_shot_delay(0), 0);
        assert_eq!(clamp_shot_delay(250), 250);
        assert_eq!(clamp_shot_delay(1023), 1023);
        assert_eq!(clamp_shot_delay(1024), 1023);
        assert_eq!(clamp_shot_delay(i32::MAX), 1023);
    }
}
