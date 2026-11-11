//! The vision tool's application shell: the main window, the child windows it
//! opens (config, plots, aimpoint test) and the tasks that feed the shared
//! [`MotRunner`].
//!
//! The UI is immediate-mode. Nothing here keeps widget state in sync with the
//! model. Every frame reads the shared state and draws it. While anything live is
//! on screen the app asks for a repaint each frame, so it redraws at the display's
//! refresh rate and needs no fixed timer.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::Duration;

use app_dirs2::{AppDataType, get_app_root};
use arrayvec::ArrayVec;
use eframe::egui;
use eros::{Context as _, Result};
use odyssey_common::ScreenCalibration;
use odyssey_usb::device::GeneralSettings;
use odyssey_usb::packets::vm::{AccelConfig, PacketData};
use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::config_window::ConfigWindow;
use crate::consts::APP_INFO;
use crate::imaging::ImageWindow;
use crate::mot_runner::{self, MotRunner};
use crate::plots_window::PlotsWindow;
use crate::recording::encode_recording;
use crate::test_canvas::Outcome;
use crate::{MotState, TestFrame, run_canvas, run_raw_canvas, test_canvas, theme};

/// Every screen id up to and including `MAX_SCREEN_ID` can have a saved calibration.
const SCREEN_SLOTS: usize = (odyssey_common::MAX_SCREEN_ID + 1) as usize;

/// Madgwick filter gain (beta).
const MADGWICK_BETA: f32 = 0.2;

/// Default size of the main window, in points.
const MAIN_WINDOW_SIZE: [f32; 2] = [760.0, 560.0];

/// Completion polling while no display-rate visualization is visible.
const BACKGROUND_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Starts the tool and blocks until its main window closes.
///
/// # Errors
///
/// Fails if the async runtime or the window system cannot start.
///
/// # Since
/// Odyssey Desktop 0.7.0.
pub fn run(simulate: bool, legacy_sim: bool) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to start the async runtime")?;
    let _enter = runtime.enter();
    #[cfg(not(feature = "sim"))]
    if simulate || legacy_sim {
        eros::bail!(
            "--sim / --sim-legacy requires building odyssey-vision-tool with the sim feature"
        );
    }
    #[cfg(feature = "sim")]
    let simulator = if simulate || legacy_sim {
        let simulator = Arc::new(odyssey_sim::Simulator::with_sensor_model(if legacy_sim {
            odyssey_sim::SensorModel::Paj7025
        } else {
            odyssey_sim::SensorModel::Pag7665qn
        }));
        runtime.block_on(
            simulator
                .handle()
                .execute(odyssey_sim::Command::UnitUsb { plugged: true }),
        )?;
        Some(simulator)
    } else {
        None
    };
    #[cfg(feature = "sim")]
    let simulated_device = simulator
        .as_ref()
        .map(|simulator| simulator.connect_unit_usb(odyssey_sim::UNIT_UUID));
    #[cfg(not(feature = "sim"))]
    let simulated_device: Option<odyssey_usb::device::VmDevice> = None;

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
                runtime.handle().clone(),
                simulated_device.as_ref(),
                #[cfg(feature = "sim")]
                simulator.clone(),
            )))
        }),
    )
    .map_err(|e| eros::error!("the window system failed: {e}"))
}

/// What the user has switched on. Tracking, raw tracking and the aimpoint test
/// are independent, and each shows its own view.
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
        self.tracking_raw || self.tracking || self.testing || self.recording
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
    stream: LiveTask,
    raw_frames: LiveTask,
    save: Option<JoinHandle<Result<()>>>,
}

impl Drop for Tasks {
    fn drop(&mut self) {
        if let Some(save) = &self.save {
            save.abort();
        }
    }
}

/// Retains a cancelled task until it has stopped, so a replacement cannot
/// subscribe to the same device stream while teardown is still running.
#[derive(Debug, Default)]
struct LiveTask {
    handle: Option<JoinHandle<Result<()>>>,
    stopping: bool,
}

impl LiveTask {
    fn stop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort();
            self.stopping = true;
        }
    }

    fn sync(&mut self, wanted: bool, start: impl FnOnce() -> JoinHandle<Result<()>>) -> Result<()> {
        if let Some(handle) = &mut self.handle {
            let mut context = TaskContext::from_waker(Waker::noop());
            if let Poll::Ready(result) = Pin::new(handle).poll(&mut context) {
                self.handle = None;
                let stopping = std::mem::take(&mut self.stopping);
                match result {
                    Err(error) if stopping && error.is_cancelled() => {}
                    Err(error) => return Err(error.into()),
                    Ok(result) => {
                        result?;
                        if !stopping {
                            return Err(eros::error!("device stream ended"));
                        }
                    }
                }
            }
        }
        if !wanted {
            self.stop();
        } else if self.handle.is_none() {
            self.handle = Some(start());
        }
        Ok(())
    }
}

impl Drop for LiveTask {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A recording buffer has one immutable header, even if the user later loads
/// another device or configuration before saving it.
#[derive(Debug)]
struct RecordedSettings {
    config: GeneralSettings,
    device_revision: u64,
    config_revision: u64,
}

/// The application state.
#[derive(Debug)]
struct VisionTool {
    runner: Arc<Mutex<MotRunner>>,
    datapoints: Arc<Mutex<Vec<TestFrame>>>,
    packets: Arc<Mutex<Vec<(u128, PacketData)>>>,
    recorded_settings: Option<RecordedSettings>,
    config: ConfigWindow,
    imaging: ImageWindow,
    plots: PlotsWindow,
    windows: OpenWindows,
    modes: Modes,
    tasks: Tasks,
    /// Whether the aimpoint test window is a normal window rather than fullscreen.
    test_windowed: bool,
    runtime: Handle,
    #[cfg(feature = "sim")]
    simulator: Option<Arc<odyssey_sim::Simulator>>,
    #[cfg(feature = "sim")]
    simulator_generation: u64,
    /// Last device revision pushed into the runner.
    seen_device_revision: u64,
    /// Last accelerometer config the Madgwick filter was built for.
    seen_accel: Option<AccelConfig>,
    /// Failures from background work, shown in a banner until dismissed.
    notice: Option<String>,
    #[cfg(feature = "bevy")]
    bevy: crate::bevy_viewer::BevyViewer,
}

impl VisionTool {
    fn new(
        ctx: &egui::Context,
        runtime: Handle,
        simulated_device: Option<&odyssey_usb::device::VmDevice>,
        #[cfg(feature = "sim")] simulator: Option<Arc<odyssey_sim::Simulator>>,
    ) -> Self {
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
        let imaging = ImageWindow::new(runtime.clone(), ctx.clone());
        let mut config = ConfigWindow::new(
            Arc::clone(&runner),
            runtime.clone(),
            ctx.clone(),
            simulated_device.is_none(),
        );
        config.set_image_control(imaging.control());
        #[cfg(feature = "sim")]
        if let Some(device) = simulated_device {
            config.connect_simulated(device);
        }
        #[cfg(not(feature = "sim"))]
        let _ = simulated_device;
        Self {
            #[cfg(feature = "bevy")]
            bevy: crate::bevy_viewer::BevyViewer::new(Arc::clone(&runner)),
            runner,
            datapoints,
            packets,
            recorded_settings: None,
            config,
            imaging,
            plots: PlotsWindow::new(),
            windows: OpenWindows::default(),
            modes: Modes::default(),
            tasks: Tasks::default(),
            test_windowed: false,
            runtime,
            #[cfg(feature = "sim")]
            simulator_generation: simulator.as_ref().map_or(0, |simulator| {
                simulator
                    .handle()
                    .snapshot()
                    .unit(odyssey_sim::UNIT_UUID)
                    .map_or(0, |unit| unit.usb_generation)
            }),
            #[cfg(feature = "sim")]
            simulator,
            seen_device_revision: u64::MAX,
            seen_accel: None,
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
            self.tasks.stream.stop();
            self.tasks.raw_frames.stop();
            if device.is_none() {
                self.modes = Modes::default();
                self.runner.lock().record_packets = false;
            }
            self.runner.lock().device = device;
        }

        let accel = self.config.applied_accel_config();
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
        let connected = self.connected();
        let runner = &self.runner;
        let runtime = &self.runtime;
        if let Err(error) = self
            .tasks
            .stream
            .sync(self.modes.needs_stream() && connected, || {
                runtime.spawn(mot_runner::run(Arc::clone(runner)))
            })
        {
            self.notice = Some(format!("Tracking stopped: {error:#?}"));
            self.modes = Modes::default();
            self.runner.lock().record_packets = false;
        }
        if let Err(error) = self
            .tasks
            .raw_frames
            .sync(self.modes.tracking_raw && connected, || {
                runtime.spawn(mot_runner::frame_loop(Arc::clone(runner)))
            })
        {
            self.notice = Some(format!("Raw tracking stopped: {error:#?}"));
            self.modes.tracking_raw = false;
        }
        if let Some(handle) = &mut self.tasks.save {
            let mut context = TaskContext::from_waker(Waker::noop());
            if let Poll::Ready(result) = Pin::new(handle).poll(&mut context) {
                self.tasks.save = None;
                if let Err(error) = result
                    .context("save task failed")
                    .map_err(Into::into)
                    .and_then(|result| result)
                {
                    error!("Saving failed: {error:#?}");
                    self.notice = Some(format!("Saving failed: {error:#?}"));
                }
            }
        }
    }

    fn has_live_view(&self) -> bool {
        self.imaging.open
            || self.modes.tracking_raw
            || self.modes.tracking
            || self.modes.testing
            || self.windows.plots
    }

    fn has_background_work(&self) -> bool {
        let running = self.tasks.stream.handle.is_some()
            || self.tasks.raw_frames.handle.is_some()
            || self.tasks.save.is_some()
            || self.imaging.busy();
        #[cfg(feature = "bevy")]
        let running = running || self.bevy.is_running();
        running
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
    fn save_datapoints(&mut self) {
        let frames = self.datapoints.lock().clone();
        self.tasks.save = Some(self.runtime.spawn(write_datapoints(frames)));
    }

    /// Encodes the recorded packets, asks where to save, then writes them, off the UI thread.
    fn save_packets(&mut self) {
        let (config, packets) = {
            let runner = self.runner.lock();
            let config = self
                .recorded_settings
                .as_ref()
                .map_or(&runner.general_config, |settings| &settings.config);
            (config.clone(), self.packets.lock().clone())
        };
        self.tasks.save = Some(self.runtime.spawn(write_recording(config, packets)));
    }

    fn toggle_recording(&mut self) {
        if self.modes.recording {
            self.modes.recording = false;
            self.runner.lock().record_packets = false;
            return;
        }
        let device_revision = self.config.device_revision();
        let config_revision = self.config.config_revision();
        let mut runner = self.runner.lock();
        if self.packets.lock().is_empty() {
            self.recorded_settings = Some(RecordedSettings {
                config: runner.general_config.clone(),
                device_revision,
                config_revision,
            });
        } else if self.recorded_settings.as_ref().is_none_or(|settings| {
            settings.device_revision != device_revision
                || settings.config_revision != config_revision
        }) {
            self.notice = Some(
                "Clear the recording before capturing a different device or configuration."
                    .to_owned(),
            );
            return;
        }
        self.modes.recording = true;
        runner.record_packets = true;
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
            if ui
                .add_enabled(connected, egui::Button::new("Image / camera calibration"))
                .clicked()
            {
                self.imaging.open = true;
            }
            if self.config.is_synthetic() {
                ui.colored_label(egui::Color32::YELLOW, "SYNTHETIC SIMULATOR — not hardware");
            }
            #[cfg(feature = "bevy")]
            if ui.button("Launch Bevy").clicked()
                && let Err(error) = self.bevy.open()
            {
                self.notice = Some(format!("Could not open the 3D viewer: {error:#?}"));
            }
        });

        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(connected, |ui| {
                toggle(
                    ui,
                    &mut self.modes.tracking_raw,
                    "Start Raw Tracking",
                    "Stop Raw Tracking",
                );
                toggle(
                    ui,
                    &mut self.modes.tracking,
                    "Start Tracking",
                    "Stop Tracking",
                );
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
                ui.ctx().send_viewport_cmd_to(
                    test_viewport_id(),
                    egui::ViewportCommand::Decorations(self.test_windowed),
                );
            }
        });

        ui.horizontal_wrapped(|ui| {
            let label = if self.modes.recording {
                "Stop Recording"
            } else {
                "Start Recording"
            };
            if ui
                .add_enabled(
                    connected && !self.config.busy(),
                    egui::Button::selectable(self.modes.recording, label),
                )
                .clicked()
            {
                self.toggle_recording();
            }
            if ui.button("Clear").clicked() {
                self.packets.lock().clear();
            }
            if ui
                .add_enabled(self.tasks.save.is_none(), egui::Button::new("Save"))
                .clicked()
            {
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
            if ui
                .add_enabled(self.tasks.save.is_none(), egui::Button::new("Save to file"))
                .clicked()
            {
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
                    self.config.show(ui);
                    close_requested(ui)
                },
            );
            self.windows.config = !close;
        }

        if self.windows.plots {
            let close = ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("plots"),
                egui::ViewportBuilder::default()
                    .with_title("Telemetry")
                    .with_inner_size([1100.0, 800.0]),
                |ui, _class| {
                    egui::CentralPanel::default().show(ui, |ui| self.plots.show(ui));
                    close_requested(ui)
                },
            );
            self.windows.plots = !close;
        }

        if self.config.take_capture_request() {
            self.imaging.open = true;
        }

        if self.modes.testing {
            let runner = &self.runner;
            let exit = ctx.show_viewport_immediate(
                test_viewport_id(),
                egui::ViewportBuilder::default()
                    .with_title("Aimpoint Test")
                    .with_inner_size([640.0, 480.0])
                    .with_decorations(self.test_windowed)
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
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Err(error) = self.imaging.shutdown() {
            error!("Image workflow shutdown failed: {error:#?}");
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.config.poll_events();
        #[cfg(feature = "sim")]
        if let Some(simulator) = &self.simulator {
            let snapshot = simulator.handle().snapshot();
            if let Some(unit) = snapshot.unit(odyssey_sim::UNIT_UUID)
                && unit.usb
                && unit.usb_generation != self.simulator_generation
                && !self.config.busy()
            {
                self.simulator_generation = unit.usb_generation;
                self.config
                    .connect_simulated(simulator.connect_unit_usb(odyssey_sim::UNIT_UUID));
            }
            ctx.request_repaint_after(BACKGROUND_POLL_INTERVAL);
        }
        self.sync_config();
        self.imaging.poll(
            self.config.device_revision(),
            self.config.device_identity(),
            ctx,
        );
        self.sync_tasks();
        #[cfg(feature = "bevy")]
        if let Err(error) = self.bevy.poll() {
            self.notice = Some(format!("3D viewer stopped: {error:#?}"));
        }
        if self.has_live_view() {
            ctx.request_repaint();
        } else if self.has_background_work() {
            // Recording and file dialogs need completion supervision, not a
            // display-rate redraw of an otherwise idle main window.
            ctx.request_repaint_after(BACKGROUND_POLL_INTERVAL);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let connected = self.connected();
        egui::CentralPanel::default().show(ui, |ui| {
            self.show_notice(ui);
            if self.imaging.reboot_required() && !self.imaging.open {
                ui.colored_label(ui.visuals().warn_fg_color, "Legacy image mode: tracking has NOT been restored. Reopen Image / camera calibration to stop/reboot, then reconnect, or fully power-cycle the unit.");
            }
            self.show_controls(ui, connected);
            if self.modes.testing {
                self.show_dataset(ui);
            } else {
                ui.add_space(4.0);
            }
            if self.imaging.open && ui.button("Back to tracking").clicked() {
                self.imaging.open = false;
                self.imaging.control().stop();
            }
            if self.imaging.open {
                // Keep calibration in the main surface: immediate native child viewports
                // stop receiving UI passes when Wayland withholds the occluded parent's frames.
                let device = self.config.device();
                let (two_cameras, scale) = self.config.image_capabilities();
                let busy = self.config.busy() || self.modes.recording;
                let action = egui::ScrollArea::vertical()
                    .show(ui, |ui| {
                        self.imaging
                            .show(ui, device.as_ref(), two_cameras, scale, busy)
                    })
                    .inner;
                if let Some(action) = action {
                    self.config.apply_calibration(action);
                    self.windows.config = true;
                }
            } else {
                self.show_views(ui);
            }
        });
        self.show_child_windows(&ui.ctx().clone());
    }
}

/// A selectable button with allocation-free labels for both states.
fn toggle(ui: &mut egui::Ui, on: &mut bool, start: &'static str, stop: &'static str) {
    let label = if *on { stop } else { start };
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

async fn write_datapoints(frames: Vec<TestFrame>) -> Result<()> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("csv", &["csv"])
        .save_file()
        .await
    else {
        return Ok(());
    };
    let path = with_extension(file.path().to_owned(), "csv");
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut writer = csv::Writer::from_path(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        for frame in frames {
            writer.serialize(frame).context("serializing a datapoint")?;
        }
        writer
            .flush()
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    })
    .await
    .context("datapoint writer failed")?
}

async fn write_recording(config: GeneralSettings, packets: Vec<(u128, PacketData)>) -> Result<()> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("bin", &["bin"])
        .save_file()
        .await
    else {
        return Ok(());
    };
    let path = with_extension(file.path().to_owned(), "bin");
    tokio::task::spawn_blocking(move || -> Result<()> {
        let bytes = encode_recording(&config, &packets).context("encoding packets")?;
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    })
    .await
    .context("recording writer failed")?
}

/// The directory holding `screen_<id>.json` calibrations. `SCREEN_CALIBRATIONS_DIR` overrides it.
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
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                warn!(
                    "Could not read screen calibration {}: {error}",
                    path.display()
                );
                continue;
            }
        };
        match serde_json::from_reader::<_, ScreenCalibration<f32>>(file) {
            Ok(calibration) => match calibration.validate() {
                Ok(()) => {
                    info!("Loaded {}", path.display());
                    calibrations.push((id, calibration));
                }
                Err(error) => error!("Invalid screen calibration {}: {error}", path.display()),
            },
            Err(e) => error!("Failed to deserialize screen calibration: {e}"),
        }
    }
    calibrations
}

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;

    use super::*;

    #[tokio::test]
    async fn replacement_waits_for_cancelled_subscription_to_finish() {
        let mut task = LiveTask::default();
        let (started, ready) = oneshot::channel();
        task.sync(true, || {
            tokio::spawn(async {
                let _ = started.send(());
                std::future::pending::<Result<()>>().await
            })
        })
        .unwrap();
        ready.await.unwrap();
        task.stop();

        // A current-thread runtime cannot process the abort until this task yields.
        task.sync(true, || panic!("old subscription is still tearing down"))
            .unwrap();
        tokio::task::yield_now().await;

        let (started, ready) = oneshot::channel();
        task.sync(true, || {
            tokio::spawn(async {
                let _ = started.send(());
                std::future::pending::<Result<()>>().await
            })
        })
        .unwrap();
        ready.await.unwrap();
    }

    #[tokio::test]
    async fn ended_subscription_reports_failure_and_releases_slot() {
        let mut task = LiveTask::default();
        task.sync(true, || {
            tokio::spawn(async { Err(eros::error!("camera disconnected")) })
        })
        .unwrap();
        tokio::task::yield_now().await;
        let error = task
            .sync(true, || panic!("a failed stream must not silently restart"))
            .unwrap_err();
        assert_eq!(error.to_string(), "camera disconnected");
        assert!(task.handle.is_none());
    }
}
