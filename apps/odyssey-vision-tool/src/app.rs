//! The vision tool's application shell: the main window, the child windows it
//! opens (config, plots, aimpoint test) and the tasks that feed the shared
//! [`MotRunner`].
//!
//! The UI is immediate-mode. Nothing here keeps widget state in sync with the
//! model; every frame reads the shared state and draws it. While anything live is
//! on screen the app asks for a repaint each frame, so it redraws at the display's
//! refresh rate rather than a fixed timer.

use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use anyhow::{Context as _, Result, anyhow};
use app_dirs2::{AppDataType, get_app_root};
use arrayvec::ArrayVec;
use eframe::egui;
use odyssey_common::ScreenCalibration;
use odyssey_usb::device::GeneralSettings;
use odyssey_usb::packets::vm::{AccelConfig, PacketData};
use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::task::AbortHandle;
use tracing::{error, info, warn};

use crate::config_window::ConfigWindow;
use crate::consts::APP_INFO;
use crate::mot_runner::{self, MotRunner};
use crate::plots_window::PlotsWindow;
use crate::recording::encode_recording;
use crate::test_canvas::Outcome;
use crate::{MotState, TestFrame, run_canvas, run_raw_canvas, test_canvas, theme};

/// Every screen id up to and including `MAX_SCREEN_ID` can have a saved calibration.
const SCREEN_SLOTS: usize = (odyssey_common::MAX_SCREEN_ID + 1) as usize;

/// Madgwick filter gain the tool has always used.
const MADGWICK_BETA: f32 = 0.2;

/// Default size of the main window, in points.
const MAIN_WINDOW_SIZE: [f32; 2] = [760.0, 560.0];

/// Command-line choice of where the device comes from.
#[derive(Debug, Default)]
pub struct Launch {
    /// `<addr>`: connect to a simulator at this address.
    pub simulator_addr: Option<String>,
    /// `-u <addr>`: connect over UDP to this address.
    pub udp_addr: Option<String>,
}

impl Launch {
    /// Parses the arguments after the program name.
    ///
    /// # Errors
    ///
    /// Returns the usage text when the arguments are not `[]`, `<addr>` or `-u <addr>`.
    pub fn from_args(args: &[String]) -> Result<Self> {
        match args {
            [] => Ok(Self::default()),
            [flag, addr] if flag == "-u" => Ok(Self {
                simulator_addr: None,
                udp_addr: Some(addr.clone()),
            }),
            [addr] => Ok(Self {
                simulator_addr: Some(addr.clone()),
                udp_addr: None,
            }),
            _ => Err(anyhow!(
                "unrecognized arguments; usage: odyssey-vision-tool [-u <udp addr> | <simulator addr>]"
            )),
        }
    }
}

/// Starts the tool and blocks until its main window closes.
///
/// # Errors
///
/// Fails if the async runtime or the window system cannot start.
pub fn run(launch: Launch) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to start the async runtime")?;
    let _enter = runtime.enter();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ATS Vision Tool")
            .with_inner_size(MAIN_WINDOW_SIZE)
            .with_min_inner_size([560.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ATS Vision Tool",
        options,
        Box::new(|creation| {
            theme::apply(&creation.egui_ctx);
            Ok(Box::new(VisionTool::new(
                &creation.egui_ctx,
                launch,
                runtime.handle().clone(),
            )))
        }),
    )
    .map_err(|e| anyhow!("the window system failed: {e}"))
}

/// What the user has switched on. Tracking, raw tracking and the aimpoint test
/// are independent; each shows its own view.
#[derive(Debug, Default)]
struct Modes {
    tracking_raw: bool,
    tracking: bool,
    testing: bool,
    recording: bool,
}

impl Modes {
    /// The stream task feeds every live view.
    const fn needs_stream(&self) -> bool {
        self.tracking_raw || self.tracking || self.testing
    }
}

/// Which child windows are open.
#[derive(Debug, Default)]
struct OpenWindows {
    config: bool,
    plots: bool,
}

/// The background tasks that follow [`Modes`].
#[derive(Debug, Default)]
struct Tasks {
    stream: Option<AbortHandle>,
    raw_frames: Option<AbortHandle>,
}

/// Starts `slot`'s task when it is wanted and not running, aborts it when it is
/// running and no longer wanted.
fn follow(slot: &mut Option<AbortHandle>, wanted: bool, start: impl FnOnce() -> AbortHandle) {
    match (wanted, slot.is_some()) {
        (true, false) => *slot = Some(start()),
        (false, true) => {
            if let Some(handle) = slot.take() {
                handle.abort();
            }
        }
        (true, true) | (false, false) => {}
    }
}

/// The application state.
#[derive(Debug)]
pub struct VisionTool {
    runner: Arc<Mutex<MotRunner>>,
    datapoints: Arc<Mutex<Vec<TestFrame>>>,
    packets: Arc<Mutex<Vec<(u128, PacketData)>>>,
    config: ConfigWindow,
    plots: PlotsWindow,
    windows: OpenWindows,
    modes: Modes,
    tasks: Tasks,
    /// Whether the aimpoint test window is a normal window rather than fullscreen.
    test_windowed: bool,
    runtime: Handle,
    /// Last device revision pushed into the runner.
    seen_device_revision: u64,
    /// Last accelerometer config the Madgwick filter was built for.
    seen_accel: Option<AccelConfig>,
    /// Failures from background work, shown in a banner until dismissed.
    notices: mpsc::Receiver<String>,
    notice_sender: mpsc::Sender<String>,
    notice: Option<String>,
    #[cfg(feature = "bevy")]
    bevy: crate::bevy_viewer::BevyViewer,
}

impl VisionTool {
    fn new(ctx: &egui::Context, launch: Launch, runtime: Handle) -> Self {
        let datapoints = Arc::new(Mutex::new(Vec::new()));
        let packets = Arc::new(Mutex::new(Vec::new()));
        let runner = Arc::new(Mutex::new(MotRunner {
            state: MotState::default(),
            device: None,
            record_impact: false,
            record_packets: false,
            datapoints: Arc::clone(&datapoints),
            packets: Arc::clone(&packets),
            general_config: GeneralSettings::default(),
            wfnf_realign: true,
            screen_calibrations: load_screen_calibrations(),
        }));
        let config = ConfigWindow::new(
            launch.simulator_addr,
            launch.udp_addr,
            Arc::clone(&runner),
            runtime.clone(),
            ctx.clone(),
        );
        let (notice_sender, notices) = mpsc::channel();
        Self {
            #[cfg(feature = "bevy")]
            bevy: crate::bevy_viewer::BevyViewer::new(Arc::clone(&runner)),
            runner,
            datapoints,
            packets,
            config,
            plots: PlotsWindow::new(),
            windows: OpenWindows::default(),
            modes: Modes::default(),
            tasks: Tasks::default(),
            test_windowed: false,
            runtime,
            seen_device_revision: u64::MAX,
            seen_accel: None,
            notices,
            notice_sender,
            notice: None,
        }
    }

    /// Mirrors the config window's device and accelerometer settings into the runner.
    fn sync_config(&mut self) {
        let revision = self.config.device_revision();
        if revision != self.seen_device_revision {
            self.seen_device_revision = revision;
            let device = self.config.device();
            tracing::debug!(
                "device changed: {}",
                if device.is_some() {
                    "connected"
                } else {
                    "none"
                }
            );
            if device.is_none() {
                self.modes.testing = false;
            }
            self.runner.lock().device = device;
        }

        let accel = self.config.accel_config();
        if self.seen_accel != Some(accel) {
            self.seen_accel = Some(accel);
            let mut runner = self.runner.lock();
            let quat = runner.state.madgwick.quat;
            runner.state.madgwick =
                ahrs::Madgwick::new_with_quat(1. / f32::from(accel.accel_odr), MADGWICK_BETA, quat);
        }
    }

    /// Starts and stops the device-stream tasks to match [`Modes`].
    fn sync_tasks(&mut self) {
        let runner = &self.runner;
        let runtime = &self.runtime;
        follow(&mut self.tasks.stream, self.modes.needs_stream(), || {
            runtime
                .spawn(mot_runner::run(Arc::clone(runner)))
                .abort_handle()
        });
        follow(&mut self.tasks.raw_frames, self.modes.tracking_raw, || {
            runtime
                .spawn(mot_runner::frame_loop(Arc::clone(runner)))
                .abort_handle()
        });
    }

    fn drain_notices(&mut self) {
        while let Ok(message) = self.notices.try_recv() {
            error!("{message}");
            self.notice = Some(message);
        }
    }

    fn has_live_view(&self) -> bool {
        self.modes.needs_stream() || self.windows.plots
    }

    fn connected(&self) -> bool {
        self.runner.lock().device.is_some()
    }

    fn add_datapoint(&self) {
        let runner = self.runner.lock();
        let state = &runner.state;

        let gravity = state
            .orientation
            .inverse_transform_vector(&nalgebra::Vector3::z_axis());
        let cant = (f32::atan2(-gravity.z, -gravity.x) + std::f32::consts::FRAC_PI_2).to_degrees();

        let frame = TestFrame {
            fv_aimpoint_x: Some(state.fv_aimpoint.x),
            fv_aimpoint_y: Some(state.fv_aimpoint.y),
            opposite_cant: Some(cant),
            position_x: Some(state.translation_mat.x),
            position_y: Some(state.translation_mat.y),
            position_z: Some(state.translation_mat.z),
        };
        self.datapoints.lock().push(frame);
    }

    /// Asks where to save, then writes the datapoints as CSV, off the UI thread.
    fn save_datapoints(&self) {
        let frames = self.datapoints.lock().clone();
        let notices = self.notice_sender.clone();
        self.runtime.spawn_blocking(move || {
            if let Err(e) = write_datapoints(&frames) {
                // A closed receiver means the app is shutting down.
                let _ = notices.send(format!("Could not save datapoints: {e:#}"));
            }
        });
    }

    /// Encodes the recorded packets, asks where to save, then writes them, off the UI thread.
    fn save_packets(&self) {
        // Lock order is runner, then packets, as in the MotRunner tasks.
        let encoded = {
            let runner = self.runner.lock();
            let packets = runner.packets.lock();
            encode_recording(&runner.general_config, &packets)
        };
        let bytes = match encoded {
            Ok(bytes) => bytes,
            Err(e) => {
                // A closed receiver means the app is shutting down.
                let _ = self
                    .notice_sender
                    .send(format!("Could not encode packets: {e}"));
                return;
            }
        };
        let notices = self.notice_sender.clone();
        self.runtime.spawn_blocking(move || {
            if let Err(e) = write_recording(&bytes) {
                // A closed receiver means the app is shutting down.
                let _ = notices.send(format!("Could not save packets: {e:#}"));
            }
        });
    }

    fn show_notice(&mut self, ui: &mut egui::Ui) {
        let Some(message) = &self.notice else {
            return;
        };
        let mut dismissed = false;
        egui::Frame::group(ui.style())
            .fill(theme::error_fill(ui.visuals()))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(ui.visuals().error_fg_color, message);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        dismissed = ui.button("Dismiss").clicked();
                    });
                });
            });
        ui.add_space(4.0);
        if dismissed {
            self.notice = None;
        }
    }

    fn show_controls(&mut self, ui: &mut egui::Ui, connected: bool) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Config").clicked() {
                self.windows.config = true;
            }
            if ui.button("Plots").clicked() {
                self.windows.plots = true;
            }
            #[cfg(feature = "bevy")]
            if ui.button("Launch Bevy").clicked()
                && let Err(error) = self.bevy.open()
            {
                self.notice = Some(format!("Could not open the 3D viewer: {error:#}"));
            }
        });

        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(connected, |ui| {
                toggle(ui, &mut self.modes.tracking_raw, "Raw Tracking");
                toggle(ui, &mut self.modes.tracking, "Tracking");
                if ui.button("Run Test").clicked() {
                    self.modes.testing = true;
                }
            });
            let before = self.test_windowed;
            ui.checkbox(&mut self.test_windowed, "Windowed")
                .on_hover_text("Show the aimpoint test in a window instead of fullscreen");
            if self.test_windowed != before {
                ui.ctx().send_viewport_cmd_to(
                    test_viewport_id(),
                    egui::ViewportCommand::Fullscreen(!self.test_windowed),
                );
            }
        });

        ui.horizontal_wrapped(|ui| {
            let label = if self.modes.recording {
                "Stop Recording"
            } else {
                "Start Recording"
            };
            if ui.selectable_label(self.modes.recording, label).clicked() {
                self.modes.recording = !self.modes.recording;
                self.runner.lock().record_packets = self.modes.recording;
            }
            if ui.button("Clear").clicked() {
                self.packets.lock().clear();
            }
            if ui.button("Save").clicked() {
                self.save_packets();
            }
        });
    }

    fn show_dataset(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Datapoints added: {}",
                self.datapoints.lock().len()
            ));
            if ui.button("Add datapoint").clicked() {
                self.add_datapoint();
            }
            if ui.button("Remove datapoint").clicked() {
                self.datapoints.lock().pop();
            }
            if ui.button("Clear datapoints").clicked() {
                self.datapoints.lock().clear();
            }
            let mut record_impacts = self.runner.lock().record_impact;
            if ui.checkbox(&mut record_impacts, "Record impacts").changed() {
                self.runner.lock().record_impact = record_impacts;
            }
            if ui.button("Save to file").clicked() {
                self.save_datapoints();
            }
        });
        ui.separator();
    }

    /// The live tracking views, each taking an equal share of the space left.
    fn show_views(&self, ui: &mut egui::Ui) {
        let views = usize::from(self.modes.tracking_raw) + usize::from(self.modes.tracking);
        if views == 0 {
            ui.centered_and_justified(|ui| {
                ui.weak("Connect a device in Config, then start tracking.");
            });
            return;
        }
        let share = (ui.available_height() - ui.spacing().item_spacing.y * (views - 1) as f32)
            / views as f32;
        let size = egui::vec2(ui.available_width(), share);
        if self.modes.tracking_raw {
            ui.allocate_ui(size, |ui| run_raw_canvas::show(ui, &self.runner));
        }
        if self.modes.tracking {
            ui.allocate_ui(size, |ui| run_canvas::show(ui, &self.runner));
        }
    }

    /// Shows the child windows that are open and closes the ones the user dismissed.
    fn show_child_windows(&mut self, ctx: &egui::Context) {
        if self.windows.config {
            let close = ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("config"),
                egui::ViewportBuilder::default()
                    .with_title("Config")
                    .with_inner_size([520.0, 640.0]),
                |ui, _class| {
                    egui::CentralPanel::default().show(ui, |ui| self.config.show(ui));
                    close_requested(ui)
                },
            );
            self.windows.config = !close;
        }

        if self.windows.plots {
            let close = ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("plots"),
                egui::ViewportBuilder::default()
                    .with_title("le plot")
                    .with_inner_size([1100.0, 800.0]),
                |ui, _class| {
                    egui::CentralPanel::default().show(ui, |ui| self.plots.show(ui));
                    close_requested(ui)
                },
            );
            self.windows.plots = !close;
        }

        if self.modes.testing {
            let runner = &self.runner;
            let exit = ctx.show_viewport_immediate(
                test_viewport_id(),
                egui::ViewportBuilder::default()
                    .with_title("Aimpoint Test")
                    .with_inner_size([640.0, 480.0])
                    .with_decorations(false)
                    .with_fullscreen(!self.test_windowed),
                |ui, _class| {
                    let outcome = egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| test_canvas::show(ui, runner))
                        .inner;
                    matches!(outcome, Outcome::Exit) || close_requested(ui)
                },
            );
            self.modes.testing = !exit;
        }
    }
}

impl eframe::App for VisionTool {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_notices();
        #[cfg(feature = "bevy")]
        if let Err(error) = self.bevy.poll() {
            self.notice = Some(format!("3D viewer stopped: {error:#}"));
        }
        self.sync_config();
        self.sync_tasks();
        if self.has_live_view() {
            ctx.request_repaint();
        }
        #[cfg(feature = "bevy")]
        if self.bevy.is_running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let connected = self.connected();
        egui::CentralPanel::default().show(ui, |ui| {
            self.show_notice(ui);
            self.show_controls(ui, connected);
            if self.modes.testing {
                self.show_dataset(ui);
            } else {
                ui.add_space(4.0);
            }
            self.show_views(ui);
        });
        self.show_child_windows(&ui.ctx().clone());
    }
}

impl Drop for VisionTool {
    fn drop(&mut self) {
        for handle in [self.tasks.stream.take(), self.tasks.raw_frames.take()]
            .into_iter()
            .flatten()
        {
            handle.abort();
        }
    }
}

/// A selectable button whose label reads "Start …" or "Stop …".
fn toggle(ui: &mut egui::Ui, on: &mut bool, what: &str) {
    let label = if *on {
        format!("Stop {what}")
    } else {
        format!("Start {what}")
    };
    if ui.selectable_label(*on, label).clicked() {
        *on = !*on;
    }
}

fn test_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("aimpoint-test")
}

fn close_requested(ui: &egui::Ui) -> bool {
    ui.input(|input| input.viewport().close_requested())
}

/// Appends `.{extension}` unless the path already ends in it.
fn with_extension(mut path: PathBuf, extension: &str) -> PathBuf {
    if path
        .extension()
        .is_none_or(|existing| existing != extension)
    {
        path.as_mut_os_string().push(format!(".{extension}"));
    }
    path
}

fn write_datapoints(frames: &[TestFrame]) -> Result<()> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("csv", &["csv"])
        .save_file()
    else {
        return Ok(());
    };
    let path = with_extension(path, "csv");
    let mut writer =
        csv::Writer::from_path(&path).with_context(|| format!("creating {}", path.display()))?;
    for frame in frames {
        writer.serialize(frame).context("serializing a datapoint")?;
    }
    writer
        .flush()
        .with_context(|| format!("writing {}", path.display()))
}

fn write_recording(bytes: &[u8]) -> Result<()> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("bin", &["bin"])
        .save_file()
    else {
        return Ok(());
    };
    let path = with_extension(path, "bin");
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// The directory holding `screen_<id>.json` calibrations; `SCREEN_CALIBRATIONS_DIR` overrides it.
fn screens_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("SCREEN_CALIBRATIONS_DIR") {
        return Some(dir.into());
    }
    match get_app_root(AppDataType::UserConfig, &APP_INFO) {
        Ok(root) => Some(root.join("screens")),
        Err(e) => {
            warn!("Failed to load app config: {e}");
            None
        }
    }
}

/// Loads every saved screen calibration, skipping missing or unreadable ones.
fn load_screen_calibrations() -> ArrayVec<(u8, ScreenCalibration<f32>), SCREEN_SLOTS> {
    let mut calibrations = ArrayVec::new();
    let Some(dir) = screens_dir() else {
        return calibrations;
    };
    for id in 0..=odyssey_common::MAX_SCREEN_ID {
        let path = dir.join(format!("screen_{id}.json"));
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        match serde_json::from_reader(file) {
            Ok(calibration) => {
                info!("Loaded {}", path.display());
                calibrations.push((id, calibration));
            }
            Err(e) => error!("Failed to deserialize screen calibration: {e}"),
        }
    }
    calibrations
}
