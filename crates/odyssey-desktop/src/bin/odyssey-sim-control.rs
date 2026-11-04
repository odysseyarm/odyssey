//! Manual controls and a scenario client for the simulator owned by a running hub.
//! This binary never creates a Simulator: every read and command crosses hub IPC.
#![expect(
    unused_qualifications,
    reason = "false positive on rsx! event-handler expansion"
)]
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "scenario snapshots, usage and errors go to the terminal"
)]

use std::path::PathBuf;
use std::time::Duration;

use dioxus::desktop::{Config, WindowBuilder, WindowCloseBehaviour};
use dioxus::prelude::*;
use futures::StreamExt as _;
use odyssey_hub_server::sim;
use odyssey_sim::{Command, ScenarioStep, Snapshot};

const USAGE: &str = r#"Odyssey simulator controls (requires a running simulator-enabled hub)

Usage:
  odyssey-sim-control --sim                    Open the manual control window
  odyssey-sim-control --sim --snapshot         Print the live simulator state
  odyssey-sim-control --sim --scenario FILE    Run an ordered JSON scenario
  odyssey-sim-control --help

ODYSSEY_SIM=1 (also true/yes/on) can replace --sim. The running hub must also
have explicitly enabled simulation; this program never starts a separate model.

A scenario is a JSON array. Each after_ms delay precedes its command, relative
to the preceding step, in simulator time. Steps run in file order and stop on
failure. The final authoritative snapshot is printed as JSON; errors exit 1.
Example:
[
  {"after_ms":0,"command":{"type":"unit_usb","plugged":true}},
  {"after_ms":0,"command":{"type":"dongle_usb","plugged":true}},
  {"after_ms":0,"command":{"type":"pair","timeout_ms":30000}},
  {"after_ms":100,"command":{"type":"unit_usb","plugged":false}},
  {"after_ms":50,"command":{"type":"aim","x":0.5,"y":0.5}},
  {"after_ms":10,"command":{"type":"shot"}}
]

Commands: unit_usb {plugged}, dongle_usb {plugged}, ble_range {in_range},
pair {timeout_ms}, cancel_pairing, clear_bonds, aim {x,y} (normalized 0..1),
motion {accel:[x,y,z],gyro:[x,y,z]}, shot, battery {percent,charging},
advance {millis}. All command objects include their snake_case "type".
"#;

#[derive(Default)]
enum Mode {
    #[default]
    Window,
    Snapshot,
    Scenario(PathBuf),
    Help,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("odyssey-sim-control: {error:#?}");
        std::process::exit(1);
    }
}

fn run() -> eros::Result<()> {
    let mut enabled = std::env::var("ODYSSEY_SIM").is_ok_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    });
    let mut mode = Mode::Window;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sim" => enabled = true,
            "--help" | "-h" => mode = Mode::Help,
            "--snapshot" if matches!(mode, Mode::Window) => mode = Mode::Snapshot,
            "--scenario" if matches!(mode, Mode::Window) => {
                let path = args
                    .next()
                    .ok_or_else(|| eros::error!("--scenario requires a JSON file"))?;
                mode = Mode::Scenario(path.into());
            }
            _ => {
                return Err(eros::error!(
                    "unknown or conflicting argument '{arg}'; use --help"
                ));
            }
        }
    }
    if matches!(mode, Mode::Help) {
        println!("{USAGE}");
        return Ok(());
    }
    if !enabled {
        return Err(eros::error!(
            "simulation is opt-in; pass --sim or set ODYSSEY_SIM=1"
        ));
    }
    if matches!(mode, Mode::Window) {
        let config = Config::default()
            .with_menu(None)
            .with_close_behaviour(WindowCloseBehaviour::WindowCloses)
            .with_window(
                WindowBuilder::new()
                    .with_title("Odyssey Simulator Controls")
                    .with_inner_size(dioxus::desktop::LogicalSize::new(800.0, 900.0)),
            );
        LaunchBuilder::new().with_cfg(config).launch(control_window);
        return Ok(());
    }
    let steps = if let Mode::Scenario(path) = mode {
        let bytes = eros::Context::with_context(std::fs::read(&path), || {
            format!("cannot read scenario {}", path.display())
        })?;
        Some(eros::Context::with_context(
            serde_json::from_slice::<Vec<ScenarioStep>>(&bytes),
            || format!("invalid scenario {}", path.display()),
        )?)
    } else {
        None
    };
    let runtime = eros::Context::context(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
        "cannot start IPC runtime",
    )?;
    let snapshot = eros::Context::context(
        runtime.block_on(async {
            if let Some(steps) = steps {
                sim::run_scenario(&steps).await
            } else {
                sim::snapshot().await
            }
        }),
        "simulator IPC failed",
    )?;
    println!(
        "{}",
        eros::Context::context(
            serde_json::to_string_pretty(&snapshot),
            "cannot encode simulator snapshot"
        )?
    );
    Ok(())
}

fn submit(queue: Coroutine<Command>, mut result: Signal<String>, command: eros::Result<Command>) {
    match command {
        Ok(command) => queue.send(command),
        Err(error) => result.set(error.to_string()),
    }
}

fn number(value: &str, name: &str) -> eros::Result<f32> {
    value
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| eros::error!("{name} must be a finite number"))
}

#[component]
fn NumberField(label: &'static str, mut value: Signal<String>) -> Element {
    rsx! {
        label { class: "number-field",
            {label}
            input { r#type: "number", step: "any", value: "{value}",
                oninput: move |event| value.set(event.value())
            }
        }
    }
}

fn control_window() -> Element {
    let mut snapshot = use_signal(|| None::<Snapshot>);
    let mut connection_error = use_signal(String::new);
    let mut result =
        use_signal(|| "Commands below are sent to the running simulator hub.".to_owned());
    let queue = use_coroutine(move |mut commands: UnboundedReceiver<Command>| async move {
        let mut poll = tokio::time::interval(Duration::from_millis(250));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                command = commands.next() => {
                    let Some(command) = command else { break };
                    match sim::request(command).await {
                        Ok(state) => {
                            snapshot.set(Some(state));
                            connection_error.set(String::new());
                            result.set("Command accepted by the hub".to_owned());
                        }
                        Err(error) => result.set(format!("Command failed: {error}")),
                    }
                }
                _ = poll.tick() => {
                    match sim::snapshot().await {
                        Ok(state) => {
                            snapshot.set(Some(state));
                            connection_error.set(String::new());
                        }
                        Err(error) => connection_error.set(format!("Hub unavailable: {error}. Start the hub with simulation enabled. Displayed state, if any, is the last received snapshot.")),
                    }
                }
            }
        }
    });
    let timeout = use_signal(|| "30000".to_owned());
    let aim_x = use_signal(|| "0.5".to_owned());
    let aim_y = use_signal(|| "0.5".to_owned());
    let accel_x = use_signal(|| "0".to_owned());
    let accel_y = use_signal(|| "0".to_owned());
    let accel_z = use_signal(|| "9.81".to_owned());
    let gyro_x = use_signal(|| "0".to_owned());
    let gyro_y = use_signal(|| "0".to_owned());
    let gyro_z = use_signal(|| "0".to_owned());
    let percent = use_signal(|| "100".to_owned());
    let mut charging = use_signal(|| false);
    let advance = use_signal(|| "1000".to_owned());
    let current = snapshot.read();
    let known = current.is_some() && connection_error.read().is_empty();
    let unit_usb = current.as_ref().is_some_and(|state| state.unit_usb);
    let dongle_usb = current.as_ref().is_some_and(|state| state.dongle_usb);
    let in_range = current.as_ref().is_some_and(|state| state.ble_in_range);
    let state_text = match current.as_ref() {
        Some(state) => serde_json::to_string_pretty(state)
            .unwrap_or_else(|error| format!("Cannot display state: {error}")),
        None => "No simulator state received yet.".to_owned(),
    };
    rsx! {
        style { {STYLE} }
        main {
            h1 { "Odyssey Simulator Controls" }
            p { "Controls the simulator inside the running hub. No physical device is required. Changes below are sent over local IPC." }
            if !connection_error.read().is_empty() {
                p { class: "error", role: "alert", "{connection_error}" }
            }
            p { role: "status", "{result}" }
            fieldset { disabled: !known,
                legend { "Connections" }
                label {
                    input { r#type: "checkbox", checked: unit_usb,
                        onchange: move |event| queue.send(Command::UnitUsb { plugged: event.checked() })
                    }
                    "Unit USB plugged"
                }
                label {
                    input { r#type: "checkbox", checked: dongle_usb,
                        onchange: move |event| queue.send(Command::DongleUsb { plugged: event.checked() })
                    }
                    "Dongle USB plugged"
                }
                label {
                    input { r#type: "checkbox", checked: in_range,
                        onchange: move |event| queue.send(Command::BleRange { in_range: event.checked() })
                    }
                    "Unit within BLE range"
                }
            }
            fieldset { disabled: !known,
                legend { "Pairing and bonds" }
                NumberField { label: "Pair timeout (ms)", value: timeout }
                button { onclick: move |_| submit(queue, result, (|| {
                    let timeout_ms = eros::Context::context(timeout.read().parse::<u32>(),
                        "Pair timeout must be an unsigned 32-bit millisecond count")?;
                    Ok(Command::Pair { timeout_ms })
                })()), "Pair unit and dongle" }
                button { onclick: move |_| queue.send(Command::CancelPairing), "Cancel pairing" }
                button { onclick: move |_| queue.send(Command::ClearBonds), "Clear bonds" }
            }
            fieldset { disabled: !known,
                legend { "Aim and trigger" }
                NumberField { label: "Aim X (0–1)", value: aim_x }
                NumberField { label: "Aim Y (0–1)", value: aim_y }
                button { onclick: move |_| submit(queue, result, (|| {
                    let x = number(&aim_x.read(), "Aim X")?;
                    let y = number(&aim_y.read(), "Aim Y")?;
                    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                        return Err(eros::error!("Aim coordinates must be in 0–1"));
                    }
                    Ok(Command::Aim { x, y })
                })()), "Apply aim" }
                button { onclick: move |_| queue.send(Command::Shot), "Trigger shot" }
            }
            fieldset { disabled: !known,
                legend { "Motion" }
                div {
                    NumberField { label: "Accel X (m/s²)", value: accel_x }
                    NumberField { label: "Accel Y (m/s²)", value: accel_y }
                    NumberField { label: "Accel Z (m/s²)", value: accel_z }
                }
                div {
                    NumberField { label: "Gyro X (rad/s)", value: gyro_x }
                    NumberField { label: "Gyro Y (rad/s)", value: gyro_y }
                    NumberField { label: "Gyro Z (rad/s)", value: gyro_z }
                }
                button { onclick: move |_| submit(queue, result, (|| {
                    Ok(Command::Motion {
                        accel: [number(&accel_x.read(), "Accel X")?, number(&accel_y.read(), "Accel Y")?, number(&accel_z.read(), "Accel Z")?],
                        gyro: [number(&gyro_x.read(), "Gyro X")?, number(&gyro_y.read(), "Gyro Y")?, number(&gyro_z.read(), "Gyro Z")?],
                    })
                })()), "Apply motion" }
            }
            fieldset { disabled: !known,
                legend { "Battery" }
                NumberField { label: "Charge (0–100%)", value: percent }
                label {
                    input { r#type: "checkbox", checked: charging(), onchange: move |event| charging.set(event.checked()) }
                    "Charging"
                }
                button { onclick: move |_| submit(queue, result, (|| {
                    let percent = percent.read().parse::<u8>().ok().filter(|value| *value <= 100)
                        .ok_or_else(|| eros::error!("Battery charge must be a whole percentage from 0 to 100"))?;
                    Ok(Command::Battery { percent, charging: charging() })
                })()), "Apply battery" }
            }
            fieldset { disabled: !known,
                legend { "Simulator clock" }
                NumberField { label: "Advance (ms)", value: advance }
                button { onclick: move |_| submit(queue, result, (|| {
                    let millis = eros::Context::context(advance.read().parse::<u64>(),
                        "Advance must be an unsigned 64-bit millisecond count")?;
                    Ok(Command::Advance { millis })
                })()), "Advance time" }
            }
            h2 { "Live authoritative state (250 ms polling)" }
            p { "Numeric fields are editable drafts. This state is read from the running hub, not predicted by the controls." }
            pre { {state_text} }
        }
    }
}

const STYLE: &str = r#"
body { margin: 0; color: #17202a; background: #f3f5f8; font: 15px system-ui, sans-serif; }
main { max-width: 900px; margin: auto; padding: 20px; }
h1 { margin-top: 0; font-size: 24px; } h2 { font-size: 19px; }
p { line-height: 1.45; }
fieldset { border: 1px solid #b9c4d0; border-radius: 6px; background: white; margin: 14px 0; padding: 12px; }
legend { font-weight: 650; }
label { display: inline-flex; align-items: center; gap: 6px; margin: 5px 12px 5px 0; }
.number-field { flex-direction: column; align-items: flex-start; }
input[type=number] { width: 125px; padding: 6px; font: inherit; }
button { margin: 5px 8px 5px 0; padding: 8px 12px; border: 1px solid #7c8da0; border-radius: 4px; background: #eef4ff; color: #17202a; cursor: pointer; font: inherit; }
button:hover { background: #d8e7ff; } button:disabled { cursor: not-allowed; opacity: .5; }
.error { color: #a01515; background: #fff0f0; border: 1px solid #d98b8b; padding: 10px; }
pre { padding: 14px; border-radius: 6px; background: #17202a; color: #eff6ff; overflow: auto; font-size: 13px; }
"#;
