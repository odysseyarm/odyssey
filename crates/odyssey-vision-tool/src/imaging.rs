//! Integrated USB image inspection and native-pixel mono/stereo calibration.
//! A single supervised worker owns stream start, graceful stop and camera changes.
//! Detection and solving run on the blocking pool, never on the egui thread.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use eframe::egui;
use eros::{Context as _, Result};
use odyssey_protocol::Port;
use odyssey_protocol::image::{ImageFrame, ImageStreamConfig};
use odyssey_usb::device::VmDevice;
use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt as _;
use viewer::calibration::{self, MonoCalibration, Observation, Pattern, StereoCalibration, Target};

#[derive(Clone)]
struct Request {
    device: VmDevice,
    config: ImageStreamConfig,
    legacy: bool,
}

/// Shared with the device picker so replacing a device awaits the old sensor's
/// graceful stop, including an in-flight detector, before opening its successor.
#[derive(Clone)]
pub(crate) struct ImageControl {
    request: watch::Sender<Option<Request>>,
    gate: Arc<tokio::sync::Mutex<()>>,
    shutdown: Arc<AtomicBool>,
    reboot_required: Arc<AtomicBool>,
}
impl std::fmt::Debug for ImageControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageControl").finish_non_exhaustive()
    }
}
impl ImageControl {
    pub(crate) fn stop(&self) {
        self.request.send_replace(None);
    }
    pub(crate) async fn stop_and_wait(&self) {
        self.stop();
        let _stopped = self.gate.lock().await;
    }
    pub(crate) fn reboot_required(&self) -> bool {
        self.reboot_required.load(Ordering::Relaxed)
    }
    pub(crate) fn recovered(&self) {
        self.reboot_required.store(false, Ordering::Relaxed);
    }
}

struct Preview {
    frame: Arc<ImageFrame>,
    detected: Option<Observation>,
    detection_error: Option<String>,
    target: Target,
    received: Instant,
    fps: Option<f32>,
    color: egui::ColorImage,
}
#[derive(Default)]
struct Shared {
    latest: Option<Preview>,
    status: String,
    running: bool,
}

#[derive(Clone)]
struct Capture {
    frame: Arc<ImageFrame>,
    detected: Observation,
}

pub(crate) struct ApplyCalibration {
    pub nf: Option<String>,
    pub wf: Option<String>,
    pub stereo: Option<String>,
    pub flash: bool,
}

enum WorkResult {
    Mono(Port, MonoCalibration),
    Stereo(StereoCalibration),
    Imported(Vec<Capture>),
    Saved,
    RebootRequested,
    Cancelled,
}

pub(crate) struct ImageWindow {
    pub open: bool,
    runtime: Handle,
    control: ImageControl,
    worker: Option<JoinHandle<()>>,
    shared: Arc<Mutex<Shared>>,
    target: Arc<Mutex<Target>>,
    camera: Port,
    fps: u8,
    device_revision: u64,
    dataset_device: Option<[u8; 6]>,
    current: Option<Preview>,
    texture: Option<egui::TextureHandle>,
    nf: Vec<Capture>,
    wf: Vec<Capture>,
    pending_pair: Option<Capture>,
    pairs: Vec<(Observation, Observation)>,
    mono_nf: Option<MonoCalibration>,
    mono_wf: Option<MonoCalibration>,
    stereo: Option<StereoCalibration>,
    job: Option<JoinHandle<Result<WorkResult>>>,
    cancel: Arc<AtomicBool>,
    notice: String,
}
impl std::fmt::Debug for ImageWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageWindow")
            .field("open", &self.open)
            .field("camera", &self.camera)
            .finish_non_exhaustive()
    }
}
impl ImageWindow {
    pub(crate) fn new(runtime: Handle, ctx: egui::Context) -> Self {
        let (request, receiver) = watch::channel(None);
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let reboot_required = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Mutex::new(Shared::default()));
        let target = Arc::new(Mutex::new(Target::default()));
        let worker = runtime.spawn(image_worker(
            receiver,
            Arc::clone(&gate),
            Arc::clone(&shutdown),
            Arc::clone(&reboot_required),
            Arc::clone(&shared),
            Arc::clone(&target),
            ctx,
        ));
        Self {
            open: false,
            runtime,
            control: ImageControl {
                request,
                gate,
                shutdown,
                reboot_required,
            },
            worker: Some(worker),
            shared,
            target,
            camera: Port::Nf,
            fps: 15,
            device_revision: u64::MAX,
            dataset_device: None,
            current: None,
            texture: None,
            nf: Vec::new(),
            wf: Vec::new(),
            pending_pair: None,
            pairs: Vec::new(),
            mono_nf: None,
            mono_wf: None,
            stereo: None,
            job: None,
            cancel: Arc::new(AtomicBool::new(false)),
            notice: String::new(),
        }
    }
    pub(crate) fn control(&self) -> ImageControl {
        self.control.clone()
    }
    pub(crate) fn busy(&self) -> bool {
        self.job.is_some() || self.shared.lock().running
    }
    pub(crate) fn reboot_required(&self) -> bool {
        self.control.reboot_required()
    }

    /// Called at the application boundary while the Tokio runtime is still
    /// alive. Native calls cannot be aborted safely: retain and join them, and
    /// await the sensor's explicit stop before the window/runtime is destroyed.
    pub(crate) fn shutdown(&mut self) -> Result<()> {
        if self.worker.is_none() && self.job.is_none() {
            return Ok(());
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.control.shutdown.store(true, Ordering::Relaxed);
        self.control.stop();
        let worker = self.worker.take();
        let job = self.job.take();
        let (worker_result, job_result) = self.runtime.block_on(async {
            self.control.stop_and_wait().await;
            let worker_result: Result<()> = if let Some(worker) = worker {
                worker
                    .await
                    .context("Image session worker failed")
                    .map_err(Into::into)
            } else {
                Ok(())
            };
            let job_result: Result<()> = if let Some(job) = job {
                match job.await.context("Calibration worker failed") {
                    Ok(result) => result.map(|_| ()),
                    Err(error) => Err(error.into()),
                }
            } else {
                Ok(())
            };
            (worker_result, job_result)
        });
        // Both joins have completed before propagating either error.
        worker_result?;
        if let Err(error) = job_result {
            tracing::warn!("Calibration work ended during shutdown: {error:#?}");
        }
        let state = self.shared.lock();
        if state.status.starts_with("Failed to restore") {
            eros::bail!("{}", state.status);
        }
        Ok(())
    }
    pub(crate) fn poll(&mut self, revision: u64, identity: Option<[u8; 6]>, ctx: &egui::Context) {
        if revision != self.device_revision {
            self.control.stop();
            self.cancel.store(true, Ordering::Relaxed);
            self.device_revision = revision;
            // A reboot/reconnect of the same unit must retain fitted calibration
            // and NF/WF captures. Never carry them to a different physical unit.
            if let Some(identity) = identity {
                if self.dataset_device != Some(identity) {
                    self.reset();
                    self.camera = Port::Nf;
                }
                self.dataset_device = Some(identity);
            }
            self.texture = None;
            self.current = None;
            let mut shared = self.shared.lock();
            shared.latest = None;
            if identity.is_some() && !self.control.reboot_required() {
                shared.status = "Device connected in object mode; preview stopped.".into();
            }
        }
        if !self.open {
            self.control.stop();
            if self.job.is_some() {
                self.cancel.store(true, Ordering::Relaxed);
            }
        }
        if let Some(preview) = self.shared.lock().latest.take() {
            let Preview {
                frame,
                detected,
                detection_error,
                target,
                received,
                fps,
                color,
            } = preview;
            if let Some(texture) = &mut self.texture {
                texture.set(color, egui::TextureOptions::NEAREST);
            } else {
                self.texture = Some(ctx.load_texture(
                    "live-calibration-image",
                    color,
                    egui::TextureOptions::NEAREST,
                ));
            }
            self.current = Some(Preview {
                frame,
                detected,
                detection_error,
                target,
                received,
                fps,
                color: egui::ColorImage::default(),
            });
        }
        if self.job.as_ref().is_some_and(JoinHandle::is_finished) {
            let Some(mut job) = self.job.take() else {
                return;
            };
            let waker = std::task::Waker::noop();
            let mut context = std::task::Context::from_waker(waker);
            if let std::task::Poll::Ready(result) = std::pin::Pin::new(&mut job).poll(&mut context)
            {
                if !self.cancel.load(Ordering::Relaxed) {
                    match result {
                        Ok(Ok(WorkResult::Mono(Port::Nf, result))) => self.mono_nf = Some(result),
                        Ok(Ok(WorkResult::Mono(Port::Wf, result))) => self.mono_wf = Some(result),
                        Ok(Ok(WorkResult::Stereo(result))) => self.stereo = Some(result),
                        Ok(Ok(WorkResult::Imported(captures))) => {
                            self.mono_nf = None;
                            self.mono_wf = None;
                            self.stereo = None;
                            for capture in captures {
                                if capture.frame.camera == Port::Nf {
                                    self.nf.push(capture);
                                } else {
                                    self.wf.push(capture);
                                }
                            }
                        }
                        Ok(Ok(WorkResult::Saved)) => self.notice = "Export completed".into(),
                        Ok(Ok(WorkResult::Cancelled)) => self.notice = "Operation cancelled".into(),
                        Ok(Ok(WorkResult::RebootRequested)) => self.notice = "Reboot acknowledged. Recovery completes only after USB returns and configuration reconnect succeeds. If it does not recover, fully power-cycle it; unplugging USB alone may not power off a battery-powered unit.".into(),
                        Ok(Err(error)) => self.notice = format!("{error:#?}"),
                        Err(error) => self.notice = format!("Calibration worker failed: {error}"),
                    }
                }
            } else {
                self.job = Some(job);
            }
        }
        if self.busy() || self.open {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }
    fn reset(&mut self) {
        self.nf.clear();
        self.wf.clear();
        self.pairs.clear();
        self.pending_pair = None;
        self.mono_nf = None;
        self.mono_wf = None;
        self.stereo = None;
    }
    fn start(&mut self, device: &VmDevice, legacy: bool) {
        self.current = None;
        self.texture = None;
        self.shared.lock().latest = None;
        if legacy {
            self.control.reboot_required.store(true, Ordering::Relaxed);
        }
        self.control.request.send_replace(Some(Request {
            device: device.clone(),
            legacy,
            config: ImageStreamConfig {
                camera: self.camera,
                fps: self.fps,
            },
        }));
    }
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        device: Option<&VmDevice>,
        two_cameras: bool,
        scale: (f64, f64),
        config_busy: bool,
    ) -> Option<ApplyCalibration> {
        let mut apply = None;
        ui.heading("Image inspection & camera calibration");
        ui.label("USB-only native grayscale images; BLE image transport is not supported.");
        if two_cameras {
            ui.colored_label(ui.visuals().warn_fg_color, "Legacy PAJ7025: entering image mode stops tracking and IMU reports. Stop, close, error or disconnect stops pixels only. Reboot/power-cycle and reconnect are REQUIRED to restore tracking; camera changes require recovery too. Unsaved sensor settings are not flashed.");
        } else {
            ui.weak("Modern camera: image-only capture pauses tracking until Stop; IMU/control remain available.");
        }
        let reboot_required = self.control.reboot_required();
        if reboot_required {
            ui.colored_label(ui.visuals().warn_fg_color, "Legacy recovery required before tracking or applying/flashing calibration. Saved settings are restored at boot; unsaved sensor edits must be reapplied after reconnect. Captures and fits are retained for this device.");
            if ui
                .add_enabled(
                    device.is_some() && self.job.is_none(),
                    egui::Button::new("Stop preview & reboot legacy device"),
                )
                .clicked()
                && let Some(device) = device
            {
                let device = device.clone();
                let control = self.control.clone();
                self.begin_job();
                self.job = Some(self.runtime.spawn(async move {
                    control.stop_and_wait().await;
                    device.reboot().await.context(
                        "Reboot failed; fully power-cycle and reconnect the legacy device",
                    )?;
                    Ok(WorkResult::RebootRequested)
                }));
            }
            ui.weak("If reboot is unavailable or the device does not return, fully power-cycle it and reconnect/reselect it in configuration. Reboot acknowledgement is not proof of restored tracking.");
        }
        ui.horizontal(|ui| {
            let previous = self.camera;
            ui.selectable_value(&mut self.camera, Port::Nf, "Near field");
            ui.add_enabled_ui(two_cameras, |ui| {
                ui.selectable_value(&mut self.camera, Port::Wf, "Wide field");
            });
            if self.camera != previous {
                self.control.stop();
                self.current = None;
                self.texture = None;
                self.shared.lock().latest = None;
            }
            ui.label("Requested FPS");
            ui.add(egui::DragValue::new(&mut self.fps).range(1..=15));
            if ui
                .add_enabled(
                    device.is_some() && !config_busy && !reboot_required,
                    egui::Button::new(if two_cameras {
                        "Enter legacy preview (reboot required)"
                    } else {
                        "Start / restart preview"
                    }),
                )
                .clicked()
                && let Some(device) = device
            {
                self.start(device, two_cameras);
            }
            if ui.button("Stop preview").clicked() {
                self.control.stop();
            }
        });
        ui.label(self.shared.lock().status.as_str());
        if let Some(preview) = &self.current {
            ui.label(format!(
                "{}×{} · frame {} · image clock {} µs · age {:.1}s · observed {} FPS",
                preview.frame.width,
                preview.frame.height,
                preview.frame.frame_id,
                preview.frame.timestamp_us,
                preview.received.elapsed().as_secs_f32(),
                preview
                    .fps
                    .map_or_else(|| "—".into(), |fps| format!("{fps:.1}"))
            ));
            if let Some(error) = &preview.detection_error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if let Some(texture) = &self.texture {
                let response = ui.add(egui::Image::new(texture).fit_to_exact_size(egui::vec2(
                    f32::from(preview.frame.width) * 2.0,
                    f32::from(preview.frame.height) * 2.0,
                )));
                if let Some(points) = &preview.detected {
                    for p in &points.image {
                        let position = response.rect.min
                            + egui::vec2(
                                (p[0] + 0.5) / f32::from(preview.frame.width)
                                    * response.rect.width(),
                                (p[1] + 0.5) / f32::from(preview.frame.height)
                                    * response.rect.height(),
                            );
                        ui.painter().circle_stroke(
                            position,
                            3.0,
                            egui::Stroke::new(1.0, egui::Color32::GREEN),
                        );
                    }
                    ui.label(format!(
                        "Target found: {} native-pixel points",
                        points.image.len()
                    ));
                } else {
                    ui.label("Target not found in this frame");
                }
            }
        }
        let mut target = self.target.lock().clone();
        let before = target.clone();
        ui.add_enabled_ui(self.job.is_none(),|ui| {
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("calibration-pattern").selected_text(target.pattern.label()).show_ui(ui,|ui| { for pattern in [Pattern::AprilGrid,Pattern::Chessboard,Pattern::SymmetricCircles,Pattern::AsymmetricCircles] { ui.selectable_value(&mut target.pattern,pattern,pattern.label()); } });
                ui.label("Rows"); ui.add(egui::DragValue::new(&mut target.rows).range(2..=64));
                ui.label("Columns"); ui.add(egui::DragValue::new(&mut target.cols).range(2..=64));
                ui.label("Pitch (mm)"); ui.add(egui::DragValue::new(&mut target.pitch_mm).range(0.01..=1000.0));
                if target.pattern!=Pattern::Chessboard { ui.label(if target.pattern==Pattern::AprilGrid {"Tag side (mm)"} else {"Circle diameter (mm)"}); ui.add(egui::DragValue::new(&mut target.feature_mm).range(0.01..=1000.0)); }
            });
            ui.weak("Chessboard dimensions count inner corners. Asymmetric circle pitch is diagonal centre spacing. AprilGrid uses T36H11B1 tags.");
        });
        if target != before {
            *self.target.lock() = target.clone();
            self.reset();
            self.notice = "Target changed: previous captures and fits cleared".into();
        }
        if let Err(error) = target.validate() {
            ui.colored_label(ui.visuals().error_fg_color, error.to_string());
        }
        ui.separator();
        ui.label(format!(
            "Captures: NF {} / WF {} · paired poses {}",
            self.nf.len(),
            self.wf.len(),
            self.pairs.len()
        ));
        ui.add_enabled_ui(self.job.is_none(), |ui| {
            ui.horizontal_wrapped(|ui| {
                let usable=self.current.as_ref().is_some_and(|p| p.detected.is_some() && p.target==target && p.received.elapsed()<Duration::from_secs(2));
                if ui.add_enabled(usable,egui::Button::new("Capture target")).clicked() { self.capture(false); }
                if ui.add_enabled(usable && two_cameras,egui::Button::new(if self.pending_pair.is_some() {"Complete stereo pair"} else {"Begin stereo pair"})).clicked() { self.capture(true); }
                if ui.button("Reset captures / fits").clicked() { self.reset(); }
                if ui.add_enabled(device.is_some(),egui::Button::new("Import target images…")).clicked() {
                    self.import_images(target.clone(), if two_cameras {(98,98)} else {(320,240)});
                }
                if ui.add_enabled(!self.nf.is_empty() || !self.wf.is_empty(),egui::Button::new("Export captures…")).clicked() { self.export_captures(); }
            });
            if let Some(first)=&self.pending_pair { ui.label(format!("Keep target AND device stationary. Switch to {} camera, start preview, then Complete stereo pair.",if first.frame.camera==Port::Nf {"wide-field"} else {"near-field"})); }
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(self.nf.len()>=3,egui::Button::new("Solve NF intrinsics")).clicked() { self.solve_mono(Port::Nf,target.clone()); }
                if ui.add_enabled(two_cameras && self.wf.len()>=3,egui::Button::new("Solve WF intrinsics")).clicked() { self.solve_mono(Port::Wf,target.clone()); }
                if ui.add_enabled(two_cameras && self.pairs.len()>=3 && self.mono_nf.is_some() && self.mono_wf.is_some(),egui::Button::new("Solve stereo")).clicked() { self.solve_stereo(); }
                if ui.add_enabled(two_cameras && self.nf.len()>=3 && self.nf.len()==self.wf.len(),egui::Button::new("Pair NF/WF datasets by index")).clicked() {
                    self.pairs=self.wf.iter().zip(&self.nf).map(|(w,n)|(w.detected.clone(),n.detected.clone())).collect();
                    self.stereo=None;
                    self.notice="Paired datasets by index. Only valid when each matching NF/WF image depicts the same stationary board pose.".into();
                }
            });
        });
        for (label, result) in [("NF", &self.mono_nf), ("WF", &self.mono_wf)] {
            if let Some(result) = result {
                ui.label(format!(
                    "{label}: RMS {:.4} native pixels · {} views",
                    result.rms,
                    result.view_errors.len()
                ));
                ui.collapsing(format!("{label} per-capture fit errors"), |ui| {
                    for (i, error) in result.view_errors.iter().enumerate() {
                        ui.label(format!("Capture {}: {:.4} px", i + 1, error));
                    }
                });
            }
        }
        if let Some(result) = &self.stereo {
            ui.label(format!(
                "Stereo: RMS {:.4} native pixels · {} paired poses · translation (m) {:?}",
                result.rms, result.captures, result.translation
            ));
        }
        let fitted = self.mono_nf.is_some() || self.mono_wf.is_some() || self.stereo.is_some();
        ui.add_enabled_ui(fitted && self.job.is_none(), |ui| {
            ui.horizontal(|ui| {
                if ui.button("Export calibration JSON…").clicked() {
                    self.export_calibration(scale, &target);
                }
                if ui
                    .add_enabled(
                        device.is_some() && !config_busy && !reboot_required,
                        egui::Button::new("Apply to selected device"),
                    )
                    .clicked()
                {
                    apply = self.apply(scale, &target, false);
                }
                if ui
                    .add_enabled(
                        device.is_some() && !config_busy && !reboot_required,
                        egui::Button::new("Apply & flash settings"),
                    )
                    .clicked()
                {
                    apply = self.apply(scale, &target, true);
                }
            });
        });
        if self.job.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Processing off the UI thread…");
                if ui.button("Cancel processing").clicked() {
                    self.cancel.store(true, Ordering::Relaxed);
                    self.notice =
                        "Cancelling; waiting for the current solver call to return".into();
                }
            });
        }
        if !self.notice.is_empty() {
            ui.label(&self.notice);
        }
        apply
    }
    fn capture(&mut self, pair: bool) {
        let Some(preview) = &self.current else { return };
        let Some(detected) = preview.detected.clone() else {
            return;
        };
        let capture = Capture {
            frame: Arc::clone(&preview.frame),
            detected,
        };
        let collection = if capture.frame.camera == Port::Nf {
            &mut self.nf
        } else {
            &mut self.wf
        };
        if collection.iter().any(|c| {
            c.frame.frame_id == capture.frame.frame_id
                && c.frame.timestamp_us == capture.frame.timestamp_us
        }) {
            self.notice = "This frame is already captured; wait for a new frame".into();
            return;
        }
        if pair {
            if let Some(first) = self.pending_pair.take() {
                if first.frame.camera == capture.frame.camera {
                    self.pending_pair = Some(first);
                    self.notice = "A stereo pair requires the other camera".into();
                    return;
                }
                if first.frame.width != capture.frame.width
                    || first.frame.height != capture.frame.height
                {
                    self.pending_pair = Some(first);
                    self.notice = "Stereo frame dimensions differ".into();
                    return;
                }
                let (wf, nf) = if first.frame.camera == Port::Wf {
                    (first.detected, capture.detected.clone())
                } else {
                    (capture.detected.clone(), first.detected)
                };
                self.pairs.push((wf, nf));
            } else {
                self.pending_pair = Some(capture.clone());
            }
        }
        collection.push(capture);
        self.mono_nf = None;
        self.mono_wf = None;
        self.stereo = None;
    }
    fn begin_job(&mut self) {
        self.cancel = Arc::new(AtomicBool::new(false));
        self.notice.clear();
    }
    fn solve_mono(&mut self, camera: Port, target: Target) {
        let captures = if camera == Port::Nf {
            &self.nf
        } else {
            &self.wf
        };
        let Some(first) = captures.first() else {
            return;
        };
        let resolution = (first.frame.width, first.frame.height);
        if captures
            .iter()
            .any(|c| (c.frame.width, c.frame.height) != resolution)
        {
            self.notice = "Mixed image dimensions; reset captures".into();
            return;
        }
        let views = captures
            .iter()
            .map(|c| c.detected.clone())
            .collect::<Vec<_>>();
        self.begin_job();
        let cancel = Arc::clone(&self.cancel);
        self.job = Some(self.runtime.spawn_blocking(move || {
            Ok(WorkResult::Mono(
                camera,
                calibration::solve_mono(
                    &views,
                    resolution,
                    &target,
                    if camera == Port::Nf {
                        viewer::Port::Nf
                    } else {
                        viewer::Port::Wf
                    },
                    &cancel,
                )?,
            ))
        }));
    }
    fn solve_stereo(&mut self) {
        let (Some(wf), Some(nf)) = (self.mono_wf.clone(), self.mono_nf.clone()) else {
            return;
        };
        let pairs = self.pairs.clone();
        self.begin_job();
        let cancel = Arc::clone(&self.cancel);
        self.job = Some(self.runtime.spawn_blocking(move || {
            Ok(WorkResult::Stereo(calibration::solve_stereo(
                &pairs, &wf, &nf, &cancel,
            )?))
        }));
    }
    fn apply(
        &mut self,
        scale: (f64, f64),
        target: &Target,
        flash: bool,
    ) -> Option<ApplyCalibration> {
        let json = (|| -> Result<ApplyCalibration> {
            Ok(ApplyCalibration {
                nf: self
                    .mono_nf
                    .as_ref()
                    .map(|r| r.json(scale, target))
                    .transpose()?,
                wf: self
                    .mono_wf
                    .as_ref()
                    .map(|r| r.json(scale, target))
                    .transpose()?,
                stereo: self.stereo.as_ref().map(|r| r.json(target)).transpose()?,
                flash,
            })
        })();
        match json {
            Ok(result) => Some(result),
            Err(error) => {
                self.notice = format!("{error:#?}");
                None
            }
        }
    }
    fn export_calibration(&mut self, scale: (f64, f64), target: &Target) {
        let Some(data) = self.apply(scale, target, false) else {
            return;
        };
        self.begin_job();
        let cancel = Arc::clone(&self.cancel);
        self.job = Some(self.runtime.spawn(async move {
            let folder = tokio::select! {
                ()=cancelled(&cancel)=>return Ok(WorkResult::Cancelled),
                folder=rfd::AsyncFileDialog::new().pick_folder()=>folder,
            };
            let Some(folder) = folder else {
                return Ok(WorkResult::Cancelled);
            };
            let path = folder.path().to_path_buf();
            tokio::task::spawn_blocking(move || -> Result<()> {
                for (name, json) in [
                    ("nearfield.json", data.nf),
                    ("widefield.json", data.wf),
                    ("stereo.json", data.stereo),
                ] {
                    if cancel.load(Ordering::Relaxed) {
                        eros::bail!("Export cancelled");
                    }
                    if let Some(json) = json {
                        std::fs::write(path.join(name), json)?;
                    }
                }
                Ok(())
            })
            .await
            .context("Calibration exporter failed")??;
            Ok(WorkResult::Saved)
        }));
    }
    fn import_images(&mut self, target: Target, expected_resolution: (u16, u16)) {
        let camera = self.camera;
        self.begin_job();
        let cancel = Arc::clone(&self.cancel);
        self.job=Some(self.runtime.spawn(async move {
            let files=tokio::select! {
                ()=cancelled(&cancel)=>return Ok(WorkResult::Cancelled),
                files=rfd::AsyncFileDialog::new().add_filter("Target images",&["png"]).pick_files()=>files,
            };
            let Some(mut files)=files else{return Ok(WorkResult::Cancelled)};
            files.sort_by(|a,b| a.path().cmp(b.path()));
            tokio::task::spawn_blocking(move || -> Result<WorkResult> {
                let mut captures=Vec::new();
                for (index,file) in files.iter().enumerate() {
                    if cancel.load(Ordering::Relaxed) {break;}
                    let (width,height,pixels)=calibration::load_image(file.path())?;
                    if (width,height)!=expected_resolution { eros::bail!("{} is {}×{}; selected device requires native {}×{} images",file.path().display(),width,height,expected_resolution.0,expected_resolution.1); }
                    let detected=calibration::detect(&pixels,(width,height),&target)?.ok_or_else(||eros::error!("Target not found in {}",file.path().display()))?;
                    captures.push(Capture {frame:Arc::new(ImageFrame {frame_id:index as u32,timestamp_us:0,camera,width,height,pixels,objects:Vec::new(),objects_timestamp_us:0}),detected});
                }
                Ok(WorkResult::Imported(captures))
            }).await.context("Image importer failed")?
        }));
    }
    fn export_captures(&mut self) {
        let captures = self.nf.iter().chain(&self.wf).cloned().collect::<Vec<_>>();
        self.begin_job();
        let cancel = Arc::clone(&self.cancel);
        self.job = Some(self.runtime.spawn(async move {
            let folder = tokio::select! {
                ()=cancelled(&cancel)=>return Ok(WorkResult::Cancelled),
                folder=rfd::AsyncFileDialog::new().pick_folder()=>folder,
            };
            let Some(folder) = folder else {
                return Ok(WorkResult::Cancelled);
            };
            let path = folder.path().to_path_buf();
            tokio::task::spawn_blocking(move || -> Result<()> {
                for (index, capture) in captures.iter().enumerate() {
                    if cancel.load(Ordering::Relaxed) {
                        eros::bail!("Export cancelled");
                    }
                    let camera = if capture.frame.camera == Port::Nf {
                        "nearfield"
                    } else {
                        "widefield"
                    };
                    calibration::save_image(
                        &path.join(format!("{camera}_{index:03}.png")),
                        capture.frame.width,
                        capture.frame.height,
                        &capture.frame.pixels,
                    )?;
                }
                Ok(())
            })
            .await
            .context("Capture exporter failed")??;
            Ok(WorkResult::Saved)
        }));
    }
}
impl Drop for ImageWindow {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            tracing::error!("Image workflow shutdown failed: {error:#?}");
        }
    }
}

async fn image_worker(
    mut receiver: watch::Receiver<Option<Request>>,
    gate: Arc<tokio::sync::Mutex<()>>,
    shutdown: Arc<AtomicBool>,
    reboot_required: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
    target: Arc<Mutex<Target>>,
    ctx: egui::Context,
) {
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let idle = receiver.borrow().is_none();
        if idle && receiver.changed().await.is_err() {
            break;
        }
        let guard = gate.lock().await;
        let request = receiver.borrow_and_update().clone();
        let Some(request) = request else {
            drop(guard);
            continue;
        };
        shared.lock().status = "Starting USB image session…".into();
        ctx.request_repaint();
        let mut stream = match request.device.stream_images(request.config).await {
            Ok(stream) => stream,
            Err(error) => {
                if request.legacy
                    && matches!(
                        &error,
                        odyssey_usb::Error::ImageUnsupported
                            | odyssey_usb::Error::ImageUsbRequired
                            | odyssey_usb::Error::InvalidArgument(_)
                            | odyssey_usb::Error::ImageStatus(
                                odyssey_protocol::image::ImageStatusCode::UnsupportedCamera
                            )
                    )
                {
                    reboot_required.store(false, Ordering::Relaxed);
                }
                shared.lock().status = format!("Image mode unavailable: {error}");
                shared.lock().running = false;
                drop(guard);
                ctx.request_repaint();
                if receiver.changed().await.is_err() {
                    break;
                }
                continue;
            }
        };
        shared.lock().running = true;
        shared.lock().status = if request.legacy {
            "Live legacy USB images; reboot/reconnect required to resume tracking"
        } else {
            "Live USB images; sensor object tracking paused until Stop"
        }
        .into();
        let mut previous_received = None;
        let mut request_changed = false;
        let mut renew = tokio::time::interval(Duration::from_millis(
            odyssey_protocol::image::IMAGE_LEASE_RENEW_INTERVAL_MS,
        ));
        renew.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let frame = tokio::select! {
                changed=receiver.changed()=>{if changed.is_err(){let _=stream.stop().await;return;} request_changed=true; break;},
                _=renew.tick()=>{let _=stream.renew_lease(); continue;},
                frame=stream.next()=>match frame {Some(Ok(frame))=>frame,Some(Err(error))=>{shared.lock().status=format!("Image capture stopped: {error}");break;},None=>{shared.lock().status="Image stream ended".into();break;}}
            };
            let received = Instant::now();
            let fps = previous_received.and_then(|previous: Instant| {
                let delta = received.duration_since(previous).as_secs_f32();
                (delta > 0.0).then_some(1.0 / delta)
            });
            previous_received = Some(received);
            let detector_target = target.lock().clone();
            let processed = tokio::task::spawn_blocking(move || {
                let (detected, detection_error) = match calibration::detect(
                    &frame.pixels,
                    (frame.width, frame.height),
                    &detector_target,
                ) {
                    Ok(detected) => (detected, None),
                    Err(error) => (None, Some(format!("Target detection failed: {error}"))),
                };
                let color = egui::ColorImage::from_gray(
                    [usize::from(frame.width), usize::from(frame.height)],
                    &frame.pixels,
                );
                Preview {
                    frame: Arc::new(frame),
                    detected,
                    detection_error,
                    target: detector_target,
                    received,
                    fps,
                    color,
                }
            })
            .await;
            if receiver.has_changed().unwrap_or(true) {
                request_changed = true;
                break;
            }
            match processed {
                Ok(preview) => {
                    shared.lock().latest = Some(preview);
                    ctx.request_repaint();
                }
                Err(error) => {
                    shared.lock().status = format!("Image worker failed: {error}");
                    break;
                }
            }
        }
        if let Err(error) = stream.stop().await {
            shared.lock().status = if request.legacy {
                format!(
                    "Pixels stopped; reboot/power-cycle and reconnect REQUIRED to resume tracking. Stop result: {error}"
                )
            } else {
                format!("Failed to restore sensor object mode: {error}")
            };
        } else {
            let mut state = shared.lock();
            if request.legacy {
                state.status =
                    "Pixels stopped; reboot/power-cycle and reconnect REQUIRED to resume tracking"
                        .into();
            } else if state.status.starts_with("Live USB") {
                state.status = "Stopped; sensor object tracking restored".into();
            }
        }
        shared.lock().running = false;
        drop(guard);
        ctx.request_repaint();
        if !request_changed && receiver.changed().await.is_err() {
            break;
        }
    }
}

async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
