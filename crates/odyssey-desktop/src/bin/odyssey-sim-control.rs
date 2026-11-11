//! Manual controls and a scenario client for the simulator owned by a running hub.
//! This binary never creates a Simulator: every read and command crosses hub IPC.
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "scenario snapshots, usage and errors go to the terminal"
)]

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::component::button::Button;
use gpui::component::checkbox::Checkbox;
use gpui::component::input::{Input, InputState};
use gpui::component::{Disableable as _, Sizable as _};
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, WindowOptions, div,
    px, rgb, size,
};
use odyssey_hub_server::sim::Control;
use odyssey_sim::{
    Command, ScenarioStep, Snapshot, Target, Uuid, dongle_uuid, unit_uuid, uuid_text,
};

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
A step acts on the first unit and dongle unless it names others by address.
Example:
[
  {"after_ms":0,"command":{"type":"unit_usb","plugged":true}},
  {"after_ms":0,"command":{"type":"dongle_usb","plugged":true}},
  {"after_ms":0,"command":{"type":"pair","timeout_ms":30000}},
  {"after_ms":100,"command":{"type":"unit_usb","plugged":false}},
  {"after_ms":50,"command":{"type":"aim","x":0.5,"y":0.5}},
  {"after_ms":10,"command":{"type":"shot"}},
  {"after_ms":0,"command":{"type":"add_unit","uuid":"53494d000101"}},
  {"after_ms":0,"unit":"53494d000101","command":{"type":"unit_usb","plugged":true}}
]

Commands: add_unit {uuid}, remove_unit {uuid}, add_dongle {uuid},
remove_dongle {uuid} (addresses are twelve hex digits starting 53494d),
unit_usb {plugged}, dongle_usb {plugged}, ble_range {in_range},
pair {timeout_ms}, cancel_pairing, clear_bonds, screens {screens},
pose {position_m:[x,y,z],orientation_xyzw:[x,y,z,w],at_ms?} (at_ms: simulator time to be there; default now), aim {x,y} (normalized
0..1), motion {accel:[x,y,z],gyro:[x,y,z]}, shot, battery {percent,charging},
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
        gpui::application()
            .with_assets(gpui::assets::Assets)
            .run(open_control_window);
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
            let mut control = Control::connect().await?;
            if let Some(steps) = steps {
                control.run_scenario(steps).await
            } else {
                control.snapshot().await
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

fn number(value: &str, name: &str) -> eros::Result<f32> {
    value
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| eros::error!("{name} must be a finite number"))
}

fn pair_command(timeout: &str) -> eros::Result<Command> {
    let timeout_ms = eros::Context::context(
        timeout.trim().parse::<u32>(),
        "Pair timeout must be an unsigned 32-bit millisecond count",
    )?;
    Ok(Command::Pair { timeout_ms })
}

fn aim_command(x: &str, y: &str) -> eros::Result<Command> {
    let x = number(x, "Aim X")?;
    let y = number(y, "Aim Y")?;
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return Err(eros::error!("Aim coordinates must be in 0–1"));
    }
    Ok(Command::Aim { x, y })
}

fn motion_command(v: [&str; 6]) -> eros::Result<Command> {
    Ok(Command::Motion {
        accel: [
            number(v[0], "Accel X")?,
            number(v[1], "Accel Y")?,
            number(v[2], "Accel Z")?,
        ],
        gyro: [
            number(v[3], "Gyro X")?,
            number(v[4], "Gyro Y")?,
            number(v[5], "Gyro Z")?,
        ],
    })
}

fn battery_command(percent: &str, charging: bool) -> eros::Result<Command> {
    let percent = percent
        .trim()
        .parse::<u8>()
        .ok()
        .filter(|value| *value <= 100)
        .ok_or_else(|| eros::error!("Battery charge must be a whole percentage from 0 to 100"))?;
    Ok(Command::Battery { percent, charging })
}

fn advance_command(millis: &str) -> eros::Result<Command> {
    let millis = eros::Context::context(
        millis.trim().parse::<u64>(),
        "Advance must be an unsigned 64-bit millisecond count",
    )?;
    Ok(Command::Advance { millis })
}

/// Tokio drives the hub IPC futures; GPUI's executor has no reactor.
static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the tokio runtime must start to talk to the hub")
});

/// What the IPC worker reports back to the window.
enum Update {
    State(Snapshot, Option<String>),
    Result(String),
    Unavailable(String),
}

/// One worker serializes every command and polls the authoritative state.
async fn worker(
    mut commands: futures::channel::mpsc::UnboundedReceiver<(Target, Command)>,
    updates: UnboundedSender<Update>,
) {
    let mut poll = tokio::time::interval(Duration::from_millis(250));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut hub: Option<Control> = None;
    loop {
        let command = tokio::select! {
            command = commands.next() => {
                let Some(command) = command else { break };
                Some(command)
            }
            _ = poll.tick() => None,
        };
        let accepted = command.is_some();
        let state = async {
            let control = match hub.take() {
                Some(control) => control,
                None => Control::connect().await?,
            };
            let control = hub.insert(control);
            if let Some((target, command)) = command {
                control.execute(target, command).await?;
            }
            control.snapshot().await
        }
        .await;
        let update = match state {
            Ok(state) => Update::State(
                state,
                accepted.then(|| "Command accepted by the hub".to_owned()),
            ),
            Err(odyssey_hub_server::sim::ControlError::Model(error)) => {
                Update::Result(format!("Command failed: {error}"))
            }
            Err(error) => {
                hub = None;
                Update::Unavailable(format!(
                    "Hub unavailable: {error}. Start the hub with simulation enabled. Displayed state, if any, is the last received snapshot."
                ))
            }
        };
        if updates.unbounded_send(update).is_err() {
            break;
        }
    }
}

/// A blank address field means the first unit or dongle.
fn address(text: &str, name: &str) -> eros::Result<Option<Uuid>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    uuid_text::parse(&text.to_ascii_lowercase())
        .map(Some)
        .ok_or_else(|| eros::error!("{name} address must be twelve hex digits"))
}

fn named(text: &str, name: &str) -> eros::Result<Uuid> {
    address(text, name)?.ok_or_else(|| eros::error!("Enter the {name} address first"))
}

/// The typed address, or else the first simulated address not in `taken`.
fn new_address(text: &str, name: &str, taken: &[Uuid], at: fn(u8) -> Uuid) -> eros::Result<Uuid> {
    if let Some(uuid) = address(text, name)? {
        return Ok(uuid);
    }
    (0..=u8::MAX)
        .map(at)
        .find(|uuid| !taken.contains(uuid))
        .ok_or_else(|| eros::error!("Every {name} address is taken"))
}

const CANVAS: u32 = 0xfaf8f7;
const SURFACE: u32 = 0xffffff;
const SURFACE_ALT: u32 = 0xf1eeed;
const LINE: u32 = 0xe5e0df;
const FG: u32 = 0x2b2727;
const FG_STRONG: u32 = 0x111111;
const DANGER_INK: u32 = 0xb42823;
const DANGER_SOFT: u32 = 0xfde8e7;

struct Controls {
    snapshot: Option<Snapshot>,
    connection_error: String,
    result: String,
    queue: UnboundedSender<(Target, Command)>,
    /// The unit and dongle the controls below act on.
    target: Target,
    unit: Entity<InputState>,
    dongle: Entity<InputState>,
    timeout: Entity<InputState>,
    aim_x: Entity<InputState>,
    aim_y: Entity<InputState>,
    motion: [Entity<InputState>; 6],
    percent: Entity<InputState>,
    advance: Entity<InputState>,
    charging: bool,
}

fn field(initial: &str, window: &mut Window, cx: &mut Context<'_, Controls>) -> Entity<InputState> {
    let input = cx.new(|cx| InputState::new(window, cx));
    input.update(cx, |state, cx| {
        state.set_value(initial.to_owned(), window, cx);
    });
    input
}

fn open_control_window(cx: &mut gpui::App) {
    gpui::init(cx);
    let options = WindowOptions {
        titlebar: Some(gpui::TitlebarOptions {
            title: Some("Odyssey Simulator Controls".into()),
            ..Default::default()
        }),
        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
            None,
            size(px(800.0), px(900.0)),
            cx,
        ))),
        ..Default::default()
    };
    let opened = gpui::open_window(options, cx, |window, cx| {
        cx.new(|cx| Controls::new(window, cx))
    });
    if let Err(error) = opened {
        eprintln!("odyssey-sim-control: cannot open window: {error:#}");
        cx.quit();
        return;
    }
    cx.on_window_closed(|cx, _| cx.quit()).detach();
}

impl Controls {
    fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let (queue, commands) = unbounded();
        let (updates, mut received) = unbounded();
        RUNTIME.spawn(worker(commands, updates));
        cx.spawn(async move |this, cx| {
            while let Some(update) = received.next().await {
                let applied = this.update(cx, |this, cx| {
                    match update {
                        Update::State(state, result) => {
                            this.snapshot = Some(state);
                            this.connection_error.clear();
                            if let Some(result) = result {
                                this.result = result;
                            }
                        }
                        Update::Result(result) => this.result = result,
                        Update::Unavailable(error) => this.connection_error = error,
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            snapshot: None,
            connection_error: String::new(),
            result: "Commands below are sent to the running simulator hub.".to_owned(),
            queue,
            target: Target::default(),
            unit: field("", window, cx),
            dongle: field("", window, cx),
            timeout: field("30000", window, cx),
            aim_x: field("0.5", window, cx),
            aim_y: field("0.5", window, cx),
            motion: [
                field("0", window, cx),
                field("0", window, cx),
                field("9.81", window, cx),
                field("0", window, cx),
                field("0", window, cx),
                field("0", window, cx),
            ],
            percent: field("100", window, cx),
            advance: field("1000", window, cx),
            charging: false,
        }
    }

    fn send(&mut self, command: Command) {
        // The worker only stops with the window.
        let _ = self.queue.unbounded_send((self.target, command));
    }

    fn submit(&mut self, command: eros::Result<Command>, cx: &mut Context<'_, Self>) {
        match command {
            Ok(command) => self.send(command),
            Err(error) => {
                self.result = error.to_string();
                cx.notify();
            }
        }
    }
}

fn text(input: &Entity<InputState>, cx: &gpui::App) -> String {
    input.read(cx).value().to_string()
}

fn number_field(label: &'static str, input: &Entity<InputState>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .mr_3()
        .w(px(150.0))
        .child(label)
        .child(Input::new(input).small())
}

fn group(title: &'static str, children: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .my_2()
        .rounded_lg()
        .border_1()
        .border_color(rgb(LINE))
        .bg(rgb(SURFACE))
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(FG_STRONG))
                .child(title),
        )
        .child(children)
}

impl Render for Controls {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let known = self
            .snapshot
            .as_ref()
            .filter(|_| self.connection_error.is_empty());
        let unit = known.and_then(|state| match self.target.unit {
            Some(uuid) => state.unit(uuid),
            None => state.units.first(),
        });
        let dongle = known.and_then(|state| match self.target.dongle {
            Some(uuid) => state.dongle(uuid),
            None => state.dongles.first(),
        });
        let unit_usb = unit.is_some_and(|unit| unit.usb);
        let dongle_usb = dongle.is_some_and(|dongle| dongle.usb);
        let in_range = unit.is_some_and(|unit| unit.ble_in_range);
        let known = known.is_some();
        let state_text = match self.snapshot.as_ref() {
            Some(state) => serde_json::to_string_pretty(state)
                .unwrap_or_else(|error| format!("Cannot display state: {error}")),
            None => "No simulator state received yet.".to_owned(),
        };
        let button = |id: &'static str, label: &'static str| {
            Button::new(id).label(label).small().disabled(!known)
        };
        let row = || div().flex().flex_row().flex_wrap().items_center().gap_2();
        let fixed = |command: Command| {
            move |this: &mut Self,
                  _: &gpui::ClickEvent,
                  _: &mut Window,
                  _: &mut Context<'_, Self>| {
                this.send(command.clone());
            }
        };

        div()
            .id("sim-controls")
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(CANVAS))
            .text_color(rgb(FG))
            .text_sm()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p_5()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(FG_STRONG))
                            .child("Odyssey Simulator Controls"),
                    )
                    .child("Controls the simulator inside the running hub. No physical device is required. Changes below are sent over local IPC.")
                    .children((!self.connection_error.is_empty()).then(|| {
                        div()
                            .my_2()
                            .p_2()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(DANGER_INK))
                            .bg(rgb(DANGER_SOFT))
                            .text_color(rgb(DANGER_INK))
                            .child(SharedString::from(self.connection_error.clone()))
                    }))
                    .child(div().my_2().child(SharedString::from(self.result.clone())))
                    .child(group(
                        "Devices",
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child("Addresses are twelve hex digits starting 53494d. Blank means the first unit or dongle; Add picks the next free address.")
                            .child(
                                row()
                                    .child(number_field("Unit address", &self.unit))
                                    .child(number_field("Dongle address", &self.dongle)),
                            )
                            .child(
                                row()
                                    .child(button("select", "Control these").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            let target = address(&text(&this.unit, cx), "Unit")
                                                .and_then(|unit| {
                                                    Ok(Target {
                                                        unit,
                                                        dongle: address(
                                                            &text(&this.dongle, cx),
                                                            "Dongle",
                                                        )?,
                                                    })
                                                });
                                            match target {
                                                Ok(target) => {
                                                    this.target = target;
                                                    this.result = "The controls below act on these devices.".to_owned();
                                                }
                                                Err(error) => this.result = error.to_string(),
                                            }
                                            cx.notify();
                                        }),
                                    ))
                                    .child(button("add-unit", "Add unit").on_click(cx.listener(
                                        |this, _, _, cx| {
                                            let taken: Vec<Uuid> = this.snapshot.iter().flat_map(|state| &state.units).map(|unit| unit.uuid).collect();
                                            let command = new_address(&text(&this.unit, cx), "unit", &taken, unit_uuid)
                                                .map(|uuid| Command::AddUnit { uuid });
                                            this.submit(command, cx);
                                        },
                                    )))
                                    .child(button("remove-unit", "Remove unit").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            let command = named(&text(&this.unit, cx), "unit")
                                                .map(|uuid| Command::RemoveUnit { uuid });
                                            this.submit(command, cx);
                                        }),
                                    ))
                                    .child(button("add-dongle", "Add dongle").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            let taken: Vec<Uuid> = this.snapshot.iter().flat_map(|state| &state.dongles).map(|dongle| dongle.uuid).collect();
                                            let command = new_address(&text(&this.dongle, cx), "dongle", &taken, dongle_uuid)
                                                .map(|uuid| Command::AddDongle { uuid });
                                            this.submit(command, cx);
                                        }),
                                    ))
                                    .child(button("remove-dongle", "Remove dongle").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            let command = named(&text(&this.dongle, cx), "dongle")
                                                .map(|uuid| Command::RemoveDongle { uuid });
                                            this.submit(command, cx);
                                        }),
                                    )),
                            ),
                    ))
                    .child(group(
                        "Connections",
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                Checkbox::new("unit-usb")
                                    .label("Unit USB plugged")
                                    .checked(unit_usb)
                                    .disabled(!known)
                                    .on_click(cx.listener(|this, checked: &bool, _, _| {
                                        this.send(Command::UnitUsb { plugged: *checked });
                                    })),
                            )
                            .child(
                                Checkbox::new("dongle-usb")
                                    .label("Dongle USB plugged")
                                    .checked(dongle_usb)
                                    .disabled(!known)
                                    .on_click(cx.listener(|this, checked: &bool, _, _| {
                                        this.send(Command::DongleUsb { plugged: *checked });
                                    })),
                            )
                            .child(
                                Checkbox::new("ble-range")
                                    .label("Unit within BLE range")
                                    .checked(in_range)
                                    .disabled(!known)
                                    .on_click(cx.listener(|this, checked: &bool, _, _| {
                                        this.send(Command::BleRange { in_range: *checked });
                                    })),
                            ),
                    ))
                    .child(group(
                        "Pairing and bonds",
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(number_field("Pair timeout (ms)", &self.timeout))
                            .child(
                                row()
                                    .child(button("pair", "Pair unit and dongle").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            let command = pair_command(&text(&this.timeout, cx));
                                            this.submit(command, cx);
                                        }),
                                    ))
                                    .child(
                                        button("cancel-pairing", "Cancel pairing")
                                            .on_click(cx.listener(fixed(Command::CancelPairing))),
                                    )
                                    .child(
                                        button("clear-bonds", "Clear bonds")
                                            .on_click(cx.listener(fixed(Command::ClearBonds))),
                                    ),
                            ),
                    ))
                    .child(group(
                        "Aim and trigger",
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                row()
                                    .child(number_field("Aim X (0–1)", &self.aim_x))
                                    .child(number_field("Aim Y (0–1)", &self.aim_y)),
                            )
                            .child(
                                row()
                                    .child(button("aim", "Apply aim").on_click(cx.listener(
                                        |this, _, _, cx| {
                                            let command = aim_command(
                                                &text(&this.aim_x, cx),
                                                &text(&this.aim_y, cx),
                                            );
                                            this.submit(command, cx);
                                        },
                                    )))
                                    .child(
                                        button("shot", "Trigger shot")
                                            .on_click(cx.listener(fixed(Command::Shot))),
                                    ),
                            ),
                    ))
                    .child(group(
                        "Motion",
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                row()
                                    .child(number_field("Accel X (m/s²)", &self.motion[0]))
                                    .child(number_field("Accel Y (m/s²)", &self.motion[1]))
                                    .child(number_field("Accel Z (m/s²)", &self.motion[2])),
                            )
                            .child(
                                row()
                                    .child(number_field("Gyro X (rad/s)", &self.motion[3]))
                                    .child(number_field("Gyro Y (rad/s)", &self.motion[4]))
                                    .child(number_field("Gyro Z (rad/s)", &self.motion[5])),
                            )
                            .child(button("motion", "Apply motion").on_click(cx.listener(
                                |this, _, _, cx| {
                                    let v = this.motion.each_ref().map(|input| text(input, cx));
                                    let command = motion_command(v.each_ref().map(String::as_str));
                                    this.submit(command, cx);
                                },
                            ))),
                    ))
                    .child(group(
                        "Battery",
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(number_field("Charge (0–100%)", &self.percent))
                            .child(
                                Checkbox::new("charging")
                                    .label("Charging")
                                    .checked(self.charging)
                                    .disabled(!known)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.charging = *checked;
                                        cx.notify();
                                    })),
                            )
                            .child(button("battery", "Apply battery").on_click(cx.listener(
                                |this, _, _, cx| {
                                    let command =
                                        battery_command(&text(&this.percent, cx), this.charging);
                                    this.submit(command, cx);
                                },
                            ))),
                    ))
                    .child(group(
                        "Simulator clock",
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(number_field("Advance (ms)", &self.advance))
                            .child(button("advance", "Advance time").on_click(cx.listener(
                                |this, _, _, cx| {
                                    let command = advance_command(&text(&this.advance, cx));
                                    this.submit(command, cx);
                                },
                            ))),
                    ))
                    .child(
                        div()
                            .mt_3()
                            .text_xl()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(FG_STRONG))
                            .child("Live authoritative state (250 ms polling)"),
                    )
                    .child("Numeric fields are editable drafts. This state is read from the running hub, not predicted by the controls.")
                    .child(
                        div()
                            .mt_2()
                            .p_3()
                            .rounded_lg()
                            .bg(rgb(SURFACE_ALT))
                            .font_family("monospace")
                            .text_xs()
                            .child(SharedString::from(state_text)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_are_validated() {
        assert!(number("NaN", "x").is_err());
        assert!(number("inf", "x").is_err());
        assert!(number("1.5", "x").is_ok());
        assert!(aim_command("0.5", "1.1").is_err());
        assert!(aim_command("0", "1").is_ok());
        assert!(battery_command("101", false).is_err());
        assert!(battery_command("100", true).is_ok());
        assert!(pair_command("-1").is_err());
        assert!(advance_command("1000").is_ok());
        assert!(motion_command(["0", "0", "9.81", "0", "0", "x"]).is_err());
        assert_eq!(address("  ", "Unit").unwrap(), None);
        assert_eq!(
            address("53494D000101", "Unit").unwrap(),
            Some(odyssey_sim::unit_uuid(1))
        );
        assert!(address("53494d0001", "Unit").is_err());
        assert!(named("", "unit").is_err());
    }
}
