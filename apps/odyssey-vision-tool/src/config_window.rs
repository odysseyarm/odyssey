mod pag_sensor_settings;
mod paj_sensor_settings;

use std::fs::File;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use eframe::egui;
use nusb::{DeviceId, DeviceInfo};
use odyssey_protocol::control::device::TransportMode;
use odyssey_usb::device::{GeneralSettings, MuxDevice, ProductId, VmConnectionInfo, VmDevice};
use odyssey_usb::packets::vm::{AccelConfig, GeneralConfig, GyroConfig, Port, PropKind, Props};
use opencv_ros_camera::RosOpenCvIntrinsics;
use pag_sensor_settings::{PagAction, PagSensorSettingsForm};
use paj_sensor_settings::PajSensorSettingsForm;
use parking_lot::Mutex;

use crate::mot_runner::MotRunner;

/// Width of the label column shared by every form grid, so tabs line up.
const LABEL_WIDTH: f32 = 190.0;
/// Width of the text fields and pickers in the forms.
const FIELD_WIDTH: f32 = 220.0;
/// How long a button keeps showing "Applied!" / "Saved!".
const FLASH_DURATION: Duration = Duration::from_secs(3);
const SUCCESS_COLOR: egui::Color32 = egui::Color32::from_rgb(0x6c, 0xc0, 0x7a);
/// USB vendor ID of every Odyssey device.
const VENDOR_ID: u16 = 0x1915;
const SIMULATOR_LABEL_PREFIX: &str = "Simulator @ ";
const M4HUB_LABEL_PREFIX: &str = "M4Hub @ ";

/// The configuration window: a device picker, one tab per settings group, and
/// the actions that write the settings to the device.
///
/// Device IO runs on the tokio runtime; [`ConfigWindow::poll_events`] applies
/// results even when this window is hidden.
pub struct ConfigWindow {
    runtime: tokio::runtime::Handle,
    runner: Arc<Mutex<MotRunner>>,
    sender: Arc<EventSender>,
    task_waker: std::task::Waker,
    events: mpsc::Receiver<Event>,

    simulator_addr: Option<String>,
    udp_addr: Option<String>,
    /// Connectable devices found by the last scan.
    connections: Vec<VmConnectionInfo>,
    muxes: Vec<ExistingMux>,
    /// Discovered devices, then optional simulator and M4Hub entries.
    choices: Vec<DeviceChoice>,
    selected: Option<usize>,
    /// Identifies the latest connection attempt so that a slow, superseded
    /// attempt cannot install its device.
    connection_attempt: u64,
    operation: Option<Operation>,
    task: Option<tokio::task::JoinHandle<()>>,
    device: Option<VmDevice>,
    device_revision: u64,
    config_revision: u64,
    applied_accel_config: AccelConfig,

    tab: Tab,
    general: GeneralSettingsForm,
    wf: PajSensorSettingsForm,
    nf: PajSensorSettingsForm,
    pag: PagSensorSettingsForm,

    notice: Option<Notice>,
    flash: Option<(Flash, Instant)>,
}

impl std::fmt::Debug for ConfigWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigWindow")
            .field("selected", &self.selected)
            .field("connected", &self.device.is_some())
            .field("device_revision", &self.device_revision)
            .finish_non_exhaustive()
    }
}

impl ConfigWindow {
    /// Creates the window and starts scanning for devices.
    pub fn new(
        simulator_addr: Option<String>,
        udp_addr: Option<String>,
        runner: Arc<Mutex<MotRunner>>,
        runtime: tokio::runtime::Handle,
        ctx: egui::Context,
    ) -> Self {
        let (tx, events) = mpsc::channel();
        let sender = Arc::new(EventSender { tx, ctx });
        let mut window = Self {
            runtime,
            runner,
            task_waker: std::task::Waker::from(Arc::clone(&sender)),
            sender,
            events,
            simulator_addr,
            udp_addr,
            connections: Vec::new(),
            muxes: Vec::new(),
            choices: Vec::new(),
            selected: None,
            connection_attempt: 0,
            operation: None,
            task: None,
            device: None,
            device_revision: 0,
            config_revision: 0,
            applied_accel_config: AccelConfig::default(),
            tab: Tab::General,
            general: GeneralSettingsForm::default(),
            wf: PajSensorSettingsForm::new(Port::Wf),
            nf: PajSensorSettingsForm::new(Port::Nf),
            pag: PagSensorSettingsForm::default(),
            notice: None,
            flash: None,
        };
        window.rebuild_choices();
        window.refresh_devices();
        window
    }

    /// The connected device, if any. Cloning a device is not free; compare
    /// [`ConfigWindow::device_revision`] first.
    pub fn device(&self) -> Option<VmDevice> {
        self.device.clone()
    }

    /// Increments every time the connected device changes: connect,
    /// disconnect, or switching to another device.
    pub fn device_revision(&self) -> u64 {
        self.device_revision
    }

    /// Increments when the tracker adopts settings from a connect, reload or apply.
    pub fn config_revision(&self) -> u64 {
        self.config_revision
    }

    /// The device's accepted configuration, never an unapplied form edit.
    pub fn applied_accel_config(&self) -> AccelConfig {
        self.applied_accel_config
    }

    /// Whether async config work is pending. Recording must not start until
    /// pending device reads/writes and their UI-thread completion have finished.
    pub fn busy(&self) -> bool {
        self.operation.is_some()
    }

    /// Draws the configuration window.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.poll_events();
        self.expire_flash(ui.ctx());

        egui::Panel::top("config_device_bar").show(ui, |ui| {
            self.device_bar(ui);
            self.notice_banner(ui);
        });
        egui::Panel::bottom("config_actions").show(ui, |ui| self.action_bar(ui));
        egui::CentralPanel::default().show(ui, |ui| self.tabs(ui));
    }

    /// Applies async results independently of whether the viewport is visible.
    pub fn poll_events(&mut self) {
        let completed = self.task.as_mut().and_then(|task| {
            let mut context = std::task::Context::from_waker(&self.task_waker);
            match std::pin::Pin::new(task).poll(&mut context) {
                std::task::Poll::Ready(result) => Some(result),
                std::task::Poll::Pending => None,
            }
        });
        while let Ok(event) = self.events.try_recv() {
            self.handle(event);
        }
        if let Some(result) = completed {
            self.task = None;
            match result {
                Err(error) => {
                    self.operation = None;
                    self.notify(Notice::error(
                        "Configuration operation failed",
                        error.to_string(),
                    ));
                }
                Ok(()) if self.operation.take().is_some() => {
                    self.notify(Notice::error(
                        "Configuration operation failed",
                        "The background operation finished without returning a result.",
                    ));
                }
                Ok(()) => {}
            }
        }
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Devices(result) => {
                self.operation = None;
                match result {
                    Ok(scan) => {
                        self.connections = scan.connections;
                        self.muxes = scan.muxes;
                        self.rebuild_choices();
                    }
                    Err(error) => self.notify(Notice::error(
                        "Failed to list USB devices",
                        format!("{error:#}"),
                    )),
                }
            }
            Event::Completed { attempt, result } => {
                if attempt != self.connection_attempt {
                    return;
                }
                self.operation = None;
                match result {
                    OperationResult::Connected { device, settings } => {
                        self.install(*settings);
                        self.set_device(Some(device));
                    }
                    OperationResult::Reloaded(settings) => {
                        self.install(*settings);
                    }
                    OperationResult::Applied {
                        config,
                        flash,
                        error,
                    } => {
                        self.applied_accel_config = config.accel_config;
                        self.runner.lock().general_config = *config;
                        self.config_revision += 1;
                        if let Some(flash) = flash {
                            self.flash = Some((flash, Instant::now() + FLASH_DURATION));
                        }
                        if let Some(error) = error {
                            self.notify(error);
                        }
                    }
                    OperationResult::Uploaded { subject, upload } => {
                        match *upload {
                            Upload::Accel(config) => self.general.accel_config = config,
                            Upload::Gyro(config) => self.general.gyro_config = config,
                            Upload::NfIntrinsics(intrinsics) => {
                                self.general.nf_intrinsics = intrinsics;
                            }
                            Upload::WfIntrinsics(intrinsics) => {
                                self.general.wf_intrinsics = intrinsics;
                            }
                            Upload::StereoIso(iso) => self.general.stereo_iso = iso,
                        }
                        let noun = subject.noun();
                        self.notify(Notice::info(
                            format!("Uploaded {noun}"),
                            format!("Successfully uploaded {noun} into the form; Apply or Save to write it to the device."),
                        ));
                    }
                    OperationResult::Failed(notice) | OperationResult::Downloaded(notice) => {
                        self.notify(notice);
                    }
                    OperationResult::Finished => {}
                }
            }
        }
    }

    fn notify(&mut self, notice: Notice) {
        if notice.kind == NoticeKind::Error {
            tracing::error!("{}: {}", notice.title, notice.message.trim_end());
        }
        self.notice = Some(notice);
    }

    fn expire_flash(&mut self, ctx: &egui::Context) {
        let Some((_, until)) = self.flash else { return };
        match until.checked_duration_since(Instant::now()) {
            Some(remaining) => ctx.request_repaint_after(remaining),
            None => self.flash = None,
        }
    }

    /// Replaces the connected device and returns the previous one.
    fn set_device(&mut self, device: Option<VmDevice>) -> Option<VmDevice> {
        self.device_revision += 1;
        std::mem::replace(&mut self.device, device)
    }

    fn install(&mut self, settings: LoadedSettings) {
        self.applied_accel_config = settings.config.accel_config;
        self.runner.lock().general_config = settings.config;
        self.config_revision += 1;
        self.general = settings.general;
        match settings.sensors {
            SensorSettings::Vm { wf, nf } => {
                self.wf = *wf;
                self.nf = *nf;
            }
            SensorSettings::Pro(pag) => self.pag = pag,
            SensorSettings::Unsupported => {}
        }
    }

    fn rebuild_choices(&mut self) {
        let selected = self
            .selected
            .and_then(|index| self.choices.get(index))
            .map(DeviceChoice::key);
        self.choices.clear();
        for connection in &self.connections {
            self.choices.push(DeviceChoice {
                label: display_for_vm_connection(connection),
                connection: Some(connection.clone()),
            });
        }
        // The simulator and M4Hub entries have no connection path.
        if let Some(addr) = &self.simulator_addr {
            self.choices.push(DeviceChoice {
                label: format!("{SIMULATOR_LABEL_PREFIX}{addr}"),
                connection: None,
            });
        }
        if let Some(addr) = &self.udp_addr {
            self.choices.push(DeviceChoice {
                label: format!("{M4HUB_LABEL_PREFIX}{addr}"),
                connection: None,
            });
        }
        self.selected =
            selected.and_then(|key| self.choices.iter().position(|choice| choice.key() == key));
    }

    fn visible_tabs(&self) -> &'static [Tab] {
        match self.general.product() {
            Some(ProductId::AtsVm | ProductId::AtsLite) => {
                &[Tab::General, Tab::WideField, Tab::NearField]
            }
            Some(ProductId::AtsPro) => &[Tab::General, Tab::Pag],
            _ => &[Tab::General],
        }
    }

    /// The sensor forms that apply to the connected product.
    fn sensors(&self) -> SensorSettings {
        match self.general.product() {
            Some(ProductId::AtsVm | ProductId::AtsLite) => SensorSettings::Vm {
                wf: Box::new(self.wf.clone()),
                nf: Box::new(self.nf.clone()),
            },
            Some(ProductId::AtsPro) => SensorSettings::Pro(self.pag.clone()),
            _ => SensorSettings::Unsupported,
        }
    }

    fn device_bar(&mut self, ui: &mut egui::Ui) {
        let mut picked = None;
        let recording = self.runner.lock().record_packets;
        ui.horizontal_wrapped(|ui| {
            ui.label("Device");
            let selected_text = self
                .selected
                .and_then(|index| self.choices.get(index))
                .map_or("Select a device", |choice| choice.label.as_str());
            ui.add_enabled_ui(!recording && self.operation.is_none(), |ui| {
                egui::ComboBox::from_id_salt("config_device")
                    .width(360.0)
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        for (index, choice) in self.choices.iter().enumerate() {
                            let is_selected = self.selected == Some(index);
                            if ui.selectable_label(is_selected, &choice.label).clicked()
                                && (!is_selected || self.device.is_none())
                            {
                                picked = Some(index);
                            }
                        }
                    });
            });
            if ui
                .add_enabled(self.operation.is_none(), egui::Button::new("Refresh"))
                .clicked()
            {
                self.refresh_devices();
            }
            if let Some(operation) = self.operation {
                ui.spinner();
                ui.label(operation.label());
            }
        });
        if let Some(index) = picked {
            self.select_device(index);
        }
        if recording {
            ui.weak("Stop recording to change device settings");
        }
        ui.add_space(4.0);
    }

    fn notice_banner(&mut self, ui: &mut egui::Ui) {
        let Some(notice) = &self.notice else { return };
        let color = match notice.kind {
            NoticeKind::Error => ui.visuals().error_fg_color,
            NoticeKind::Info => SUCCESS_COLOR,
        };
        let mut dismissed = false;
        egui::Frame::group(ui.style())
            .stroke(egui::Stroke::new(1.0, color))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.colored_label(color, egui::RichText::new(&notice.title).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        dismissed = ui.small_button("Dismiss").clicked();
                    });
                });
                if !notice.message.is_empty() {
                    ui.label(notice.message.trim_end());
                }
            });
        if dismissed {
            self.notice = None;
        }
        ui.add_space(4.0);
    }

    fn action_bar(&mut self, ui: &mut egui::Ui) {
        let flash = self.flash.map(|(flash, _)| flash);
        let apply_label = if flash == Some(Flash::Applied) {
            "Applied!"
        } else {
            "Apply"
        };
        let save_label = if flash == Some(Flash::Saved) {
            "Saved!"
        } else {
            "Save"
        };
        let recording = self.runner.lock().record_packets;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.device.is_some() && self.operation.is_none(), |ui| {
                ui.add_enabled_ui(!recording, |ui| {
                    if ui.button(apply_label).clicked() {
                        self.commit(Commit::Apply);
                    }
                    if ui.button(save_label).clicked() {
                        self.commit(Commit::Save);
                    }
                    if ui.button("Reload").clicked() {
                        self.reload();
                    }
                });
                if ui.button("Load defaults").clicked() {
                    self.load_defaults();
                }
            });
        });
        ui.add_space(4.0);
    }

    fn tabs(&mut self, ui: &mut egui::Ui) {
        let tabs = self.visible_tabs();
        if !tabs.contains(&self.tab) {
            self.tab = Tab::General;
        }
        ui.horizontal(|ui| {
            for &tab in tabs {
                ui.selectable_value(&mut self.tab, tab, tab.label());
            }
        });
        ui.separator();

        let connected = self.device.is_some();
        let mut general_action = None;
        let mut pag_action = PagAction::None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_enabled_ui(self.operation.is_none(), |ui| match self.tab {
                    Tab::General => general_action = self.general.ui(ui, connected),
                    Tab::WideField => {
                        ui.add_enabled_ui(connected, |ui| self.wf.ui(ui));
                    }
                    Tab::NearField => {
                        ui.add_enabled_ui(connected, |ui| self.nf.ui(ui));
                    }
                    Tab::Pag => {
                        pag_action = ui.add_enabled_ui(connected, |ui| self.pag.ui(ui)).inner;
                    }
                });
            });

        if let Some(action) = general_action {
            self.general_action(action);
        }
        if let PagAction::CaptureImage = pag_action
            && let Some(device) = self.device.clone()
        {
            self.begin_operation(Operation::Capture);
            let sender = Arc::clone(&self.sender);
            let attempt = self.connection_attempt;
            self.task = Some(self.runtime.spawn(async move {
                let result = match PagSensorSettingsForm::capture_image(&device).await {
                    Ok(()) => OperationResult::Finished,
                    Err(error) => OperationResult::Failed(Notice::error(
                        "Failed to capture image",
                        format!("{error:#}"),
                    )),
                };
                sender.send(Event::Completed { attempt, result });
            }));
        }
    }

    fn refresh_devices(&mut self) {
        if self.operation.is_some() {
            return;
        }
        self.begin_operation(Operation::Refresh);
        let existing_muxes = self.muxes.clone();
        let sender = Arc::clone(&self.sender);
        self.task = Some(self.runtime.spawn(async move {
            sender.send(Event::Devices(scan_devices(existing_muxes).await));
        }));
    }

    fn select_device(&mut self, index: usize) {
        if self.operation.is_some()
            || self.runner.lock().record_packets
            || index >= self.choices.len()
        {
            return;
        }
        self.selected = Some(index);
        self.general.clear();
        self.wf.clear();
        self.nf.clear();
        self.pag.clear();
        let old_device = self.set_device(None);
        self.connection_attempt += 1;
        self.begin_operation(Operation::Connect);

        let attempt = self.connection_attempt;
        let target = self
            .choices
            .get(index)
            .and_then(|choice| choice.connection.clone());
        self.flash = None;
        let sender = Arc::clone(&self.sender);
        self.task = Some(self.runtime.spawn(async move {
            let result = match connect(old_device, target).await {
                Ok((device, settings)) => OperationResult::Connected {
                    device,
                    settings: Box::new(settings),
                },
                Err(error) => OperationResult::Failed(Notice::error(
                    "Failed to connect",
                    format!("{error:#}"),
                )),
            };
            sender.send(Event::Completed { attempt, result });
        }));
    }

    /// Validates the forms, writes them to the device, and for
    /// [`Commit::Save`] also flashes them.
    fn commit(&mut self, commit: Commit) {
        if self.operation.is_some() || self.runner.lock().record_packets {
            return;
        }
        let Some(device) = self.device.clone() else {
            return;
        };
        let sensors = self.sensors();
        let mut errors = vec![];
        self.general.validate(&mut errors);
        let validation =
            check_validation(&errors, "General Validation Error").and_then(|()| sensors.validate());
        if let Err(notice) = validation {
            self.notify(notice);
            return;
        }

        let general = self.general.clone();
        self.begin_operation(match commit {
            Commit::Apply => Operation::Apply,
            Commit::Save => Operation::Save,
        });
        let attempt = self.connection_attempt;
        let sender = Arc::clone(&self.sender);
        self.task = Some(self.runtime.spawn(async move {
            let result = match Box::pin(apply_settings(&device, &general, &sensors)).await {
                Err(notice) => OperationResult::Failed(notice),
                Ok(config) => {
                    let (flash, error) = match commit {
                        Commit::Apply => (Some(Flash::Applied), None),
                        Commit::Save => match device.flash_settings().await {
                            Ok(()) => (Some(Flash::Saved), None),
                            Err(error) => (
                                None,
                                Some(Notice::error(
                                    "Settings applied but not saved",
                                    format!("Failed to flash settings: {error:#}"),
                                )),
                            ),
                        },
                    };
                    OperationResult::Applied {
                        config: Box::new(config),
                        flash,
                        error,
                    }
                }
            };
            sender.send(Event::Completed { attempt, result });
        }));
    }

    fn reload(&mut self) {
        if self.operation.is_some() || self.runner.lock().record_packets {
            return;
        }
        let Some(device) = self.device.clone() else {
            return;
        };
        self.begin_operation(Operation::Reload);
        let attempt = self.connection_attempt;
        let sender = Arc::clone(&self.sender);
        self.task = Some(self.runtime.spawn(async move {
            let result = match load_settings(&device).await {
                Ok(settings) => OperationResult::Reloaded(Box::new(settings)),
                Err(error) => OperationResult::Failed(Notice::error(
                    "Failed to reload settings",
                    format!("{error:#}"),
                )),
            };
            sender.send(Event::Completed { attempt, result });
        }));
    }

    fn load_defaults(&mut self) {
        self.general.load_defaults();
        self.nf.load_defaults();
        self.wf.load_defaults();
        self.pag.load_defaults();
    }

    fn begin_operation(&mut self, operation: Operation) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.operation = Some(operation);
        self.notice = None;
        self.flash = None;
    }

    fn general_action(&mut self, action: GeneralAction) {
        match action {
            GeneralAction::Upload(document) => self.upload(document),
            GeneralAction::Download(document) => self.download(document),
            GeneralAction::SyncStereo => {
                self.general.stereo_iso = self.runner.lock().general_config.stereo_iso;
            }
        }
    }

    fn upload(&mut self, document: Document) {
        match document {
            Document::Accel => self.upload_file(Subject::Configuration, |file| {
                Ok(Upload::Accel(serde_json::from_reader(file)?))
            }),
            Document::Gyro => self.upload_file(Subject::Configuration, |file| {
                Ok(Upload::Gyro(serde_json::from_reader(file)?))
            }),
            Document::NfCalibration => self.upload_file(Subject::Calibration, |file| {
                Ok(Upload::NfIntrinsics(
                    odyssey_common::get_intrinsics_from_opencv_camera_calibration_json(file)
                        .map_err(boxed_error)?,
                ))
            }),
            Document::WfCalibration => self.upload_file(Subject::Calibration, |file| {
                Ok(Upload::WfIntrinsics(
                    odyssey_common::get_intrinsics_from_opencv_camera_calibration_json(file)
                        .map_err(boxed_error)?,
                ))
            }),
            Document::StereoCalibration => self.upload_file(Subject::Calibration, |file| {
                Ok(Upload::StereoIso(
                    odyssey_common::get_isometry_from_opencv_stereo_calibration_json(file)
                        .map_err(boxed_error)?,
                ))
            }),
        }
    }

    fn download(&mut self, document: Document) {
        match document {
            Document::Accel => {
                let config = self.general.accel_config;
                self.download_file(Subject::Configuration, move |file| {
                    Ok(serde_json::to_writer(file, &config)?)
                });
            }
            Document::Gyro => {
                let config = self.general.gyro_config;
                self.download_file(Subject::Configuration, move |file| {
                    Ok(serde_json::to_writer(file, &config)?)
                });
            }
            Document::NfCalibration => {
                let intrinsics = self.general.nf_intrinsics.clone();
                self.download_file(Subject::Calibration, move |file| {
                    odyssey_common::write_opencv_minimal_camera_calibration_json(&intrinsics, file)
                        .map_err(boxed_error)
                });
            }
            Document::WfCalibration => {
                let intrinsics = self.general.wf_intrinsics.clone();
                self.download_file(Subject::Calibration, move |file| {
                    odyssey_common::write_opencv_minimal_camera_calibration_json(&intrinsics, file)
                        .map_err(boxed_error)
                });
            }
            Document::StereoCalibration => {
                let iso = self.general.stereo_iso;
                self.download_file(Subject::Calibration, move |file| {
                    odyssey_common::write_opencv_minimal_stereo_calibration_json(&iso, file)
                        .map_err(boxed_error)
                });
            }
        }
    }

    /// Opens a cancellable async dialog, then parses the chosen file on the
    /// blocking pool. Cancelling the dialog leaves the draft unchanged.
    fn upload_file(
        &mut self,
        subject: Subject,
        read: impl FnOnce(File) -> Result<Upload> + Send + 'static,
    ) {
        if self.operation.is_some() {
            return;
        }
        self.begin_operation(Operation::Upload);
        let attempt = self.connection_attempt;
        let sender = Arc::clone(&self.sender);
        self.task = Some(self.runtime.spawn(async move {
            let Some(file) = rfd::AsyncFileDialog::new().pick_file().await else {
                sender.send(Event::Completed {
                    attempt,
                    result: OperationResult::Finished,
                });
                return;
            };
            let path = file.path().to_path_buf();
            let result = tokio::task::spawn_blocking(move || {
                File::open(&path)
                    .with_context(|| format!("Failed to open {}", path.display()))
                    .and_then(read)
            })
            .await
            .context("File reader task failed")
            .and_then(|result| result);
            let result = match result {
                Ok(upload) => OperationResult::Uploaded {
                    subject,
                    upload: Box::new(upload),
                },
                Err(error) => OperationResult::Failed(Notice::error(
                    format!("Failed to upload {}", subject.noun()),
                    format!("{error:#}"),
                )),
            };
            sender.send(Event::Completed { attempt, result });
        }));
    }

    /// Opens a cancellable async save dialog. Only the actual file write runs
    /// on the blocking pool, so an open dialog cannot hold runtime shutdown.
    fn download_file(
        &mut self,
        subject: Subject,
        write: impl FnOnce(File) -> Result<()> + Send + 'static,
    ) {
        if self.operation.is_some() {
            return;
        }
        self.begin_operation(Operation::Download);
        let attempt = self.connection_attempt;
        let sender = Arc::clone(&self.sender);
        self.task = Some(self.runtime.spawn(async move {
            let Some(file) = rfd::AsyncFileDialog::new().save_file().await else {
                sender.send(Event::Completed {
                    attempt,
                    result: OperationResult::Finished,
                });
                return;
            };
            let noun = subject.noun();
            let path = file.path().to_path_buf();
            let result = tokio::task::spawn_blocking(move || {
                File::create(&path)
                    .with_context(|| format!("Failed to create {}", path.display()))
                    .and_then(write)
            })
            .await
            .context("File writer task failed")
            .and_then(|result| result);
            let notice = match result {
                Ok(()) => Notice::info(
                    format!("Downloaded {noun}"),
                    format!("Successfully downloaded {noun}"),
                ),
                Err(error) => {
                    Notice::error(format!("Failed to download {noun}"), format!("{error:#}"))
                }
            };
            sender.send(Event::Completed {
                attempt,
                result: OperationResult::Downloaded(notice),
            });
        }));
    }
}

impl Drop for ConfigWindow {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    WideField,
    NearField,
    Pag,
}

impl Tab {
    const fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::WideField => "Wide field",
            Self::NearField => "Near field",
            Self::Pag => "PAG",
        }
    }
}

/// One entry of the device picker.
struct DeviceChoice {
    label: String,
    /// `None` for the simulator and M4Hub entries, which cannot be connected.
    connection: Option<VmConnectionInfo>,
}

#[derive(PartialEq, Eq)]
enum DeviceKey {
    Usb(DeviceId),
    Mux([u8; 6]),
    Unsupported(String),
}

impl DeviceChoice {
    fn key(&self) -> DeviceKey {
        match &self.connection {
            Some(VmConnectionInfo::DirectUsb(info)) => DeviceKey::Usb(info.id()),
            Some(VmConnectionInfo::ViaMux { device_addr, .. }) => DeviceKey::Mux(*device_addr),
            None => DeviceKey::Unsupported(self.label.clone()),
        }
    }
}

/// Device operations are single-flight: writes, reloads, scans and file uploads
/// cannot race one another or replace a form while it is being edited.
#[derive(Clone, Copy)]
enum Operation {
    Refresh,
    Connect,
    Apply,
    Save,
    Reload,
    Upload,
    Download,
    Capture,
}

impl Operation {
    const fn label(self) -> &'static str {
        match self {
            Self::Refresh => "Scanning…",
            Self::Connect => "Connecting…",
            Self::Apply => "Applying…",
            Self::Save => "Saving…",
            Self::Reload => "Reloading…",
            Self::Upload => "Importing…",
            Self::Download => "Exporting…",
            Self::Capture => "Capturing…",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Commit {
    Apply,
    Save,
}

/// Which button briefly shows a success label.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Flash {
    Applied,
    Saved,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NoticeKind {
    Error,
    Info,
}

/// A dismissable message at the top of the window.
struct Notice {
    kind: NoticeKind,
    title: String,
    message: String,
}

impl Notice {
    fn error(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: NoticeKind::Error,
            title: title.into(),
            message: message.into(),
        }
    }

    fn info(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: NoticeKind::Info,
            title: title.into(),
            message: message.into(),
        }
    }

    fn partial_apply(mut self) -> Self {
        self.message.push_str("\nSome settings may already have been written. Reload to read the device's current settings before trying again.");
        self
    }
}

/// What kind of file a transfer moves, for the messages.
#[derive(Clone, Copy)]
enum Subject {
    Calibration,
    Configuration,
}

impl Subject {
    const fn noun(self) -> &'static str {
        match self {
            Self::Calibration => "calibration",
            Self::Configuration => "configuration",
        }
    }
}

/// A file the General tab uploads and downloads.
#[derive(Clone, Copy)]
enum Document {
    Accel,
    Gyro,
    NfCalibration,
    WfCalibration,
    StereoCalibration,
}

enum GeneralAction {
    Upload(Document),
    Download(Document),
    /// Copy the stereo calibration the tracker is running with.
    SyncStereo,
}

/// A value parsed from an uploaded file.
enum Upload {
    Accel(AccelConfig),
    Gyro(GyroConfig),
    NfIntrinsics(RosOpenCvIntrinsics<f32>),
    WfIntrinsics(RosOpenCvIntrinsics<f32>),
    StereoIso(nalgebra::Isometry3<f32>),
}

/// Results of background work, applied by [`ConfigWindow::poll_events`].
enum Event {
    Devices(Result<ScannedDevices>),
    Completed {
        attempt: u64,
        result: OperationResult,
    },
}

enum OperationResult {
    Connected {
        device: VmDevice,
        settings: Box<LoadedSettings>,
    },
    Reloaded(Box<LoadedSettings>),
    Applied {
        config: Box<GeneralSettings>,
        flash: Option<Flash>,
        error: Option<Notice>,
    },
    Uploaded {
        subject: Subject,
        upload: Box<Upload>,
    },
    Failed(Notice),
    Downloaded(Notice),
    Finished,
}

/// Posts events to the window and wakes the UI up to show them.
struct EventSender {
    tx: mpsc::Sender<Event>,
    ctx: egui::Context,
}

impl EventSender {
    fn send(&self, event: Event) {
        // The receiver is gone only when the window was dropped.
        if self.tx.send(event).is_ok() {
            self.repaint();
        }
    }

    fn repaint(&self) {
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
        self.ctx
            .request_repaint_of(egui::ViewportId::from_hash_of("config"));
    }
}

// JoinHandle polling must wake the UI even when a worker panics before it can
// send an event. Keep this waker for the window's lifetime, not one per frame.
impl std::task::Wake for EventSender {
    fn wake(self: Arc<Self>) {
        self.repaint();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.repaint();
    }
}

/// Everything read from a device.
struct LoadedSettings {
    general: GeneralSettingsForm,
    sensors: SensorSettings,
    config: GeneralSettings,
}

/// The sensor forms that apply to a product.
enum SensorSettings {
    Vm {
        wf: Box<PajSensorSettingsForm>,
        nf: Box<PajSensorSettingsForm>,
    },
    Pro(PagSensorSettingsForm),
    Unsupported,
}

impl SensorSettings {
    fn validate(&self) -> Result<(), Notice> {
        let mut errors = vec![];
        match self {
            Self::Vm { wf, nf } => {
                wf.validate(&mut errors);
                check_validation(&errors, "Wide Field Validation Error")?;
                nf.validate(&mut errors);
                check_validation(&errors, "Near Field Validation Error")
            }
            Self::Pro(pag) => {
                pag.validate(&mut errors);
                check_validation(&errors, "Pag Validation Error")
            }
            Self::Unsupported => Err(Notice::error(
                "Failed to apply settings",
                "This product has no sensor settings to apply",
            )),
        }
    }

    /// Run `validate()` first.
    async fn apply(&self, device: &VmDevice) -> Result<(), Notice> {
        match self {
            Self::Vm { wf, nf } => {
                Box::pin(wf.apply(device)).await.map_err(|e| {
                    Notice::error("Failed to apply wide field settings", format!("{e:#}"))
                })?;
                Box::pin(nf.apply(device)).await.map_err(|e| {
                    Notice::error("Failed to apply near field settings", format!("{e:#}"))
                })
            }
            Self::Pro(pag) => Box::pin(pag.apply(device))
                .await
                .map_err(|e| Notice::error("Failed to apply PAG settings", format!("{e:#}"))),
            Self::Unsupported => Ok(()),
        }
    }
}

fn check_validation(errors: &[String], title: &str) -> Result<(), Notice> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Notice::error(title, validation_message(errors)))
    }
}

async fn apply_settings(
    device: &VmDevice,
    general: &GeneralSettingsForm,
    sensors: &SensorSettings,
) -> Result<GeneralSettings, Notice> {
    Box::pin(sensors.apply(device))
        .await
        .map_err(Notice::partial_apply)?;
    general.apply(device).await.map_err(|e| {
        Notice::error("Failed to apply general settings", format!("{e:#}")).partial_apply()
    })
}

/// Lays out a `label | widget` form; every tab uses the same column widths.
fn form_grid(id_salt: &'static str) -> egui::Grid {
    egui::Grid::new(id_salt)
        .num_columns(2)
        .min_col_width(LABEL_WIDTH)
        .spacing([16.0, 8.0])
        .striped(true)
}

fn form_row(ui: &mut egui::Ui, label: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.label(label);
    ui.horizontal(add_contents);
    ui.end_row();
}

#[derive(Clone)]
struct GeneralSettingsForm {
    device_uuid: [u8; 6],
    device_pid: u16,
    impact_threshold: i32,
    suppress_ms: i32,
    accel_config: AccelConfig,
    gyro_config: GyroConfig,
    nf_intrinsics: RosOpenCvIntrinsics<f32>,
    wf_intrinsics: RosOpenCvIntrinsics<f32>,
    stereo_iso: nalgebra::Isometry3<f32>,
}

impl Default for GeneralSettingsForm {
    fn default() -> Self {
        Self {
            device_uuid: [0; 6],
            device_pid: 0,
            impact_threshold: 0,
            suppress_ms: 0,
            accel_config: AccelConfig::default(),
            gyro_config: GyroConfig::default(),
            nf_intrinsics: RosOpenCvIntrinsics::from_params(145., 0., 145., 45., 45.),
            wf_intrinsics: RosOpenCvIntrinsics::from_params(34., 0., 34., 45., 45.),
            stereo_iso: nalgebra::Isometry3::identity(),
        }
    }
}

impl GeneralSettingsForm {
    fn product(&self) -> Option<ProductId> {
        ProductId::from_u16(self.device_pid)
    }

    /// Draws the General tab. The settings are editable only while `connected`;
    /// the file transfers work any time.
    fn ui(&mut self, ui: &mut egui::Ui, connected: bool) -> Option<GeneralAction> {
        let mut action = None;
        section(ui, "Device", |ui| {
            form_grid("general_device").show(ui, |ui| {
                form_row(ui, "Device UUID", |ui| {
                    if connected {
                        let id = self.device_uuid;
                        ui.label(
                            egui::RichText::new(format!(
                                "{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
                                id[0], id[1], id[2], id[3], id[4], id[5]
                            ))
                            .monospace(),
                        );
                    }
                });
                form_row(ui, "Impact threshold", |ui| {
                    ui.add_enabled(
                        connected,
                        egui::DragValue::new(&mut self.impact_threshold).range(0..=255),
                    );
                });
                form_row(ui, "Suppress (ms)", |ui| {
                    ui.add_enabled(
                        connected,
                        egui::DragValue::new(&mut self.suppress_ms).range(0..=255),
                    );
                });
            });
        });
        ui.add_space(8.0);
        section(ui, "Sensor configuration", |ui| {
            form_grid("general_sensor_config").show(ui, |ui| {
                form_row(ui, "Accelerometer Config", |ui| {
                    action = transfer_buttons(ui, Document::Accel).or(action.take());
                });
                form_row(ui, "Gyroscope Config", |ui| {
                    action = transfer_buttons(ui, Document::Gyro).or(action.take());
                });
            });
        });
        ui.add_space(8.0);
        section(ui, "Calibration", |ui| {
            form_grid("general_calibration").show(ui, |ui| {
                form_row(ui, "Nearfield Calibration", |ui| {
                    action = transfer_buttons(ui, Document::NfCalibration).or(action.take());
                });
                form_row(ui, "Widefield Calibration", |ui| {
                    action = transfer_buttons(ui, Document::WfCalibration).or(action.take());
                });
                form_row(ui, "Stereo Calibration", |ui| {
                    action = transfer_buttons(ui, Document::StereoCalibration).or(action.take());
                    if ui.button("Sync").clicked() {
                        action = Some(GeneralAction::SyncStereo);
                    }
                });
            });
        });
        action
    }

    async fn load_from_device(device: &VmDevice) -> Result<(Self, GeneralSettings)> {
        let timeout = Duration::from_millis(5000);

        let config = match retry(|| device.read_all_config(), timeout, 3).await {
            Some(Ok(cfg)) => cfg,
            Some(Err(e)) => return Err(e),
            None => {
                return Err(anyhow::anyhow!(
                    "Timeout reading device config after 3 attempts"
                ));
            }
        };

        if config.accel_config.accel_odr == 0 {
            return Err(anyhow::anyhow!(
                "Device reported a zero accelerometer output data rate"
            ));
        }

        let uuid_prop = match retry(|| device.read_prop(PropKind::Uuid), timeout, 3).await {
            Some(Ok(p)) => p,
            Some(Err(e)) => return Err(e),
            None => return Err(anyhow::anyhow!("Timeout reading UUID after 3 attempts")),
        };
        let product_id_prop =
            match retry(|| device.read_prop(PropKind::ProductId), timeout, 3).await {
                Some(Ok(p)) => p,
                Some(Err(e)) => return Err(e),
                None => {
                    return Err(anyhow::anyhow!(
                        "Timeout reading ProductId after 3 attempts"
                    ));
                }
            };

        let Props::Uuid(uuid) = uuid_prop else {
            return Err(anyhow::anyhow!("Unexpected prop variant for Uuid"));
        };
        let Props::ProductId(product_id) = product_id_prop else {
            return Err(anyhow::anyhow!("Unexpected prop variant for ProductId"));
        };
        let form = Self {
            device_uuid: uuid,
            device_pid: product_id,
            impact_threshold: i32::from(config.impact_threshold),
            suppress_ms: i32::from(config.suppress_ms),
            accel_config: config.accel_config,
            gyro_config: config.gyro_config,
            nf_intrinsics: config.camera_model_nf.clone(),
            wf_intrinsics: config.camera_model_wf.clone(),
            stereo_iso: config.stereo_iso,
        };

        Ok((form, config))
    }

    fn validate(&self, errors: &mut Vec<String>) {
        if !(0..256).contains(&self.impact_threshold) {
            errors.push("impact threshold: must be between 0 and 255".into());
        }
        if !(0..256).contains(&self.suppress_ms) {
            errors.push("suppress (ms): must be between 0 and 255".into());
        }
        if self.accel_config.accel_odr == 0 {
            errors.push("accelerometer output data rate: must be greater than zero".into());
        }
    }

    fn config(&self) -> Result<GeneralSettings> {
        Ok(GeneralSettings {
            impact_threshold: register_value(self.impact_threshold, "impact threshold")?,
            suppress_ms: register_value(self.suppress_ms, "suppress (ms)")?,
            accel_config: self.accel_config,
            gyro_config: self.gyro_config,
            camera_model_nf: self.nf_intrinsics.clone(),
            camera_model_wf: self.wf_intrinsics.clone(),
            stereo_iso: self.stereo_iso,
        })
    }

    /// Run `validate()` first: `config()` rejects only the first out-of-range
    /// field, and as an error rather than a form message.
    async fn apply(&self, device: &VmDevice) -> Result<GeneralSettings> {
        let config = self.config()?;

        // The protocol writes one `GeneralConfig` field per request.
        device
            .write_config(GeneralConfig::ImpactThreshold(config.impact_threshold))
            .await?;
        device
            .write_config(GeneralConfig::SuppressMs(config.suppress_ms))
            .await?;
        device
            .write_config(GeneralConfig::AccelConfig(config.accel_config))
            .await?;
        device
            .write_config(GeneralConfig::GyroConfig(config.gyro_config))
            .await?;
        device
            .write_config(GeneralConfig::CameraModelNf(
                (&config.camera_model_nf).into(),
            ))
            .await?;
        device
            .write_config(GeneralConfig::CameraModelWf(
                (&config.camera_model_wf).into(),
            ))
            .await?;
        device
            .write_config(GeneralConfig::StereoIso(config.stereo_iso))
            .await?;

        Ok(config)
    }

    /// Forgets the previous device: its sensor configs and product, which also
    /// hides the tabs that only apply to that product.
    fn clear(&mut self) {
        self.device_pid = 0;
        self.accel_config = AccelConfig::default();
        self.gyro_config = GyroConfig::default();
    }

    fn load_defaults(&mut self) {
        self.impact_threshold = 5;
        self.suppress_ms = 100;
        self.accel_config = AccelConfig::default();
        self.gyro_config = GyroConfig::default();

        match self.product() {
            Some(ProductId::AtsPro) => {
                // PAG7665QN sensor defaults:
                // 78° horizontal FOV, 62.7° vertical FOV, 320x240 resolution
                // With 6-bit fractional: 20480x15360 max coordinate range
                // fx = (20480/2) / tan(78°/2) ≈ 12645
                // fy = (15360/2) / tan(62.7°/2) ≈ 12601
                // cx = 20480/2 = 10240, cy = 15360/2 = 7680
                self.nf_intrinsics =
                    RosOpenCvIntrinsics::from_params(12645., 0., 12601., 10240., 7680.);
                self.wf_intrinsics = RosOpenCvIntrinsics::from_params(34., 0., 34., 45., 45.);
            }
            _ => {
                // Default NF/WF intrinsics for AtsVm, AtsLite, etc.
                self.nf_intrinsics = RosOpenCvIntrinsics::from_params(145., 0., 145., 45., 45.);
                self.wf_intrinsics = RosOpenCvIntrinsics::from_params(34., 0., 34., 45., 45.);
            }
        }
        self.stereo_iso = nalgebra::Isometry3::identity();
    }
}

/// A titled group box.
fn section(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(title).strong());
        ui.add_space(4.0);
        add_contents(ui);
    });
}

/// "Upload" and "Download" buttons for one document.
fn transfer_buttons(ui: &mut egui::Ui, document: Document) -> Option<GeneralAction> {
    let mut action = None;
    if ui.button("Upload").clicked() {
        action = Some(GeneralAction::Upload(document));
    }
    if ui.button("Download").clicked() {
        action = Some(GeneralAction::Download(document));
    }
    action
}

/// Connects to `target` after releasing `old_device`, and loads its settings.
async fn connect(
    old_device: Option<VmDevice>,
    target: Option<VmConnectionInfo>,
) -> Result<(VmDevice, LoadedSettings)> {
    // Stop the old device's streams before the new connection opens.
    if let Some(old_device) = old_device {
        tracing::debug!("Cleaning up previous device connection...");
        let _ = old_device.clear_all_streams().await;
        tracing::debug!("Previous device cleaned up (slot reservation prevents conflicts)");
    }

    let Some(target) = target else {
        return Err(anyhow::anyhow!(
            "Simulator and M4Hub connections are not supported"
        ));
    };
    tracing::debug!("Attempting to connect to device...");
    let device = target.connect().await.inspect_err(|e| {
        tracing::error!("Device connection failed: {e:?}");
    })?;

    tracing::debug!("Connection successful, disabling any active streams...");
    // Streams a previous session left enabled would flood the new connection.
    if let Err(e) = device.clear_all_streams().await {
        tracing::warn!("Failed to disable streams: {e:?}");
    }
    tracing::debug!("Streams disabled, loading general settings...");

    let settings = load_settings(&device).await?;
    tracing::debug!("All settings loaded, setting device...");
    Ok((device, settings))
}

async fn load_settings(device: &VmDevice) -> Result<LoadedSettings> {
    let (general, config) = GeneralSettingsForm::load_from_device(device)
        .await
        .inspect_err(|e| tracing::error!("Failed to load general settings: {e:?}"))?;
    tracing::debug!("General settings loaded successfully");
    let sensors = load_sensor_settings(device, general.device_pid).await?;
    Ok(LoadedSettings {
        general,
        sensors,
        config,
    })
}

/// An open mux and its last successful device list. Retaining every mux,
/// including an empty one, avoids reopening its USB interface during refresh.
#[derive(Clone)]
struct ExistingMux {
    id: DeviceId,
    mux: MuxDevice,
    device_addrs: Vec<[u8; 6]>,
}

struct ScannedDevices {
    connections: Vec<VmConnectionInfo>,
    muxes: Vec<ExistingMux>,
}

/// Lists the connectable devices: vision modules on USB, and the vision
/// modules behind every mux.
async fn scan_devices(mut existing_muxes: Vec<ExistingMux>) -> Result<ScannedDevices> {
    let usb_devices: Vec<DeviceInfo> = nusb::list_devices()
        .await?
        .filter(|info| info.vendor_id() == VENDOR_ID)
        .collect();

    // Muxes are collected separately; direct vision modules are probed for
    // their transport mode.
    let mut vm_connections: Vec<VmConnectionInfo> = Vec::new();
    let mut hub_devices: Vec<DeviceInfo> = Vec::new();

    for info in usb_devices {
        match info.product_id() {
            0x520F..=0x5211 => match VmDevice::probe_transport_mode(&info).await {
                Ok(TransportMode::Usb) => {
                    vm_connections.push(VmConnectionInfo::DirectUsb(info));
                }
                Ok(mode) => {
                    tracing::debug!(
                        "Skipping {:04x}:{:04x} - transport mode {:?}",
                        info.vendor_id(),
                        info.product_id(),
                        mode
                    );
                }
                Err(e) => {
                    tracing::debug!(
                        "Skipping {:04x}:{:04x} - failed to probe mode: {e}",
                        info.vendor_id(),
                        info.product_id()
                    );
                }
            },
            0x5212 => hub_devices.push(info),
            _ => {}
        }
    }

    tracing::debug!(
        "Found {} USB-mode devices, {} mux devices",
        vm_connections.len(),
        hub_devices.len()
    );

    let mut muxes = Vec::with_capacity(hub_devices.len());
    for hub_info in hub_devices {
        let mut existing = if let Some(index) = existing_muxes
            .iter()
            .position(|mux| mux.id == hub_info.id())
        {
            existing_muxes.swap_remove(index)
        } else {
            let id = hub_info.id();
            let mux = match MuxDevice::connect_usb(hub_info).await {
                Ok(mux) => mux,
                Err(error) => {
                    tracing::warn!("Failed to connect to mux: {error:#}");
                    continue;
                }
            };
            ExistingMux {
                id,
                mux,
                device_addrs: Vec::new(),
            }
        };
        match existing.mux.request_devices().await {
            Ok(devices) => existing.device_addrs = devices.into_iter().collect(),
            Err(error) => {
                tracing::warn!("Failed to query mux; retaining its previous devices: {error:#}");
            }
        }
        vm_connections.extend(existing.device_addrs.iter().map(|device_addr| {
            VmConnectionInfo::ViaMux {
                mux: existing.mux.clone(),
                device_addr: *device_addr,
            }
        }));
        muxes.push(existing);
    }
    Ok(ScannedDevices {
        connections: vm_connections,
        muxes,
    })
}

fn format_device_addr(addr: [u8; 6]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        addr[0], addr[1], addr[2], addr[3], addr[4], addr[5]
    )
}

fn display_for_vm_connection(conn: &VmConnectionInfo) -> String {
    match conn {
        VmConnectionInfo::DirectUsb(info) => {
            let mut out = String::new();
            if let Some(m) = info.manufacturer_string() {
                out.push_str(m);
            }
            if let Some(p) = info.product_string() {
                if !out.is_empty() {
                    out.push_str(" - ");
                }
                out.push_str(p);
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out = format!("{out}({:04x}:{:04x})", info.vendor_id(), info.product_id());
            out = out.replace('\x00', "");
            out
        }
        VmConnectionInfo::ViaMux { device_addr, .. } => {
            format!("VM via Mux ({})", format_device_addr(*device_addr))
        }
    }
}

/// Retry an asynchronous operation up to `limit` times.
async fn retry<F, G>(mut op: F, timeout: Duration, limit: usize) -> Option<G::Output>
where
    F: FnMut() -> G,
    G: Future,
{
    for _ in 0..limit {
        if let Ok(r) = tokio::time::timeout(timeout, op()).await {
            return Some(r);
        }
    }
    None
}

/// Like `retry`, but a timeout becomes an error instead of `None`, and the
/// operation's own error is converted to `anyhow::Error`.
async fn retry_checked<F, G, T, E>(op: F, timeout: Duration, limit: usize) -> Result<T>
where
    F: FnMut() -> G,
    G: Future<Output = std::result::Result<T, E>>,
    E: Into<anyhow::Error>,
{
    match retry(op, timeout, limit).await {
        Some(result) => result.map_err(Into::into),
        None => Err(anyhow::anyhow!(
            "device did not respond after {limit} attempts"
        )),
    }
}

/// Converts a UI field value to the register type it is written as.
fn register_value<T>(value: i32, name: &str) -> Result<T>
where
    T: TryFrom<i32>,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    T::try_from(value).with_context(|| format!("{name} is out of range"))
}

/// One line per validation error, for display in the error banner.
fn validation_message(errors: &[String]) -> String {
    let mut message = String::new();
    for msg in errors {
        message.push_str(msg);
        message.push('\n');
    }
    message
}

/// Loads the sensor tabs that apply to the connected product.
async fn load_sensor_settings(device: &VmDevice, product_id: u16) -> Result<SensorSettings> {
    match ProductId::from_u16(product_id) {
        Some(ProductId::AtsLite | ProductId::AtsVm) => {
            tracing::debug!("Loading WF settings...");
            let wf = PajSensorSettingsForm::load_from_device(device, Port::Wf)
                .await
                .inspect_err(|e| {
                    tracing::error!("Failed to load WF settings: {e:?}");
                })?;
            tracing::debug!("WF settings loaded, loading NF settings...");
            let nf = PajSensorSettingsForm::load_from_device(device, Port::Nf)
                .await
                .inspect_err(|e| {
                    tracing::error!("Failed to load NF settings: {e:?}");
                })?;
            tracing::debug!("NF settings loaded successfully");
            Ok(SensorSettings::Vm {
                wf: Box::new(wf),
                nf: Box::new(nf),
            })
        }
        Some(ProductId::AtsPro) => {
            tracing::debug!("Loading PAG settings...");
            let pag = PagSensorSettingsForm::load_from_device(device, product_id)
                .await
                .inspect_err(|e| {
                    tracing::error!("Failed to load PAG settings: {e:?}");
                })?;
            tracing::debug!("PAG settings loaded successfully");
            Ok(SensorSettings::Pro(pag))
        }
        Some(ProductId::Mux) | None => Ok(SensorSettings::Unsupported),
    }
}

/// `anyhow` cannot wrap the `Box<dyn Error>` the calibration file helpers return.
fn boxed_error(error: Box<dyn std::error::Error>) -> anyhow::Error {
    anyhow::anyhow!("{error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> (ConfigWindow, tokio::runtime::Runtime) {
        // This runtime is deliberately never driven: no USB scan is performed.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let runner = Arc::new(Mutex::new(MotRunner {
            state: crate::MotState::default(),
            device: None,
            general_config: GeneralSettings::default(),
            record_impact: false,
            record_packets: false,
            datapoints: Arc::new(Mutex::new(Vec::new())),
            packets: Arc::new(Mutex::new(Vec::new())),
            wfnf_realign: true,
            screen_calibrations: Default::default(),
        }));
        let mut window = ConfigWindow::new(
            None,
            None,
            runner,
            runtime.handle().clone(),
            egui::Context::default(),
        );
        window.operation = None;
        (window, runtime)
    }

    fn settings(impact_threshold: i32, odr: u16) -> LoadedSettings {
        let general = GeneralSettingsForm {
            impact_threshold,
            accel_config: AccelConfig {
                accel_odr: odr,
                ..Default::default()
            },
            ..Default::default()
        };
        let config = general.config().unwrap();
        LoadedSettings {
            general,
            config,
            sensors: SensorSettings::Unsupported,
        }
    }

    #[test]
    fn hidden_event_pump_rejects_stale_reload_without_touching_tracker() {
        let (mut window, _runtime) = window();
        window.connection_attempt = 2;
        window.operation = Some(Operation::Reload);
        let original = window.runner.lock().general_config.impact_threshold;
        window.sender.send(Event::Completed {
            attempt: 1,
            result: OperationResult::Reloaded(Box::new(settings(17, 200))),
        });
        window.poll_events();
        assert_eq!(window.config_revision(), 0);
        assert_eq!(
            window.runner.lock().general_config.impact_threshold,
            original
        );
        assert!(window.busy());

        window.sender.send(Event::Completed {
            attempt: 2,
            result: OperationResult::Reloaded(Box::new(settings(17, 200))),
        });
        window.poll_events();
        assert!(!window.busy());
        assert_eq!(window.config_revision(), 1);
        assert_eq!(window.device_revision(), 0);
        assert_eq!(window.general.impact_threshold, 17);
        assert_eq!(window.applied_accel_config().accel_odr, 200);
        assert_eq!(window.runner.lock().general_config.impact_threshold, 17);
    }

    #[test]
    fn save_failure_keeps_successfully_applied_runtime_settings() {
        let (mut window, _runtime) = window();
        window.operation = Some(Operation::Save);
        window.sender.send(Event::Completed {
            attempt: 0,
            result: OperationResult::Applied {
                config: Box::new(settings(19, 400).config),
                flash: None,
                error: Some(Notice::error("Not saved", "Flash failed")),
            },
        });
        window.poll_events();
        assert_eq!(window.runner.lock().general_config.impact_threshold, 19);
        assert_eq!(window.applied_accel_config().accel_odr, 400);
        assert_eq!(window.config_revision(), 1);
        assert!(window.flash.is_none());
        assert!(matches!(
            window.notice,
            Some(Notice {
                kind: NoticeKind::Error,
                ..
            })
        ));
        assert!(!window.busy());
    }

    #[test]
    fn uploaded_draft_does_not_change_applied_accelerometer_config() {
        let (mut window, _runtime) = window();
        let applied = window.applied_accel_config();
        window.sender.send(Event::Completed {
            attempt: 0,
            result: OperationResult::Uploaded {
                subject: Subject::Configuration,
                upload: Box::new(Upload::Accel(AccelConfig {
                    accel_odr: 800,
                    ..Default::default()
                })),
            },
        });
        window.poll_events();
        assert_eq!(window.general.accel_config.accel_odr, 800);
        assert_eq!(window.applied_accel_config(), applied);
        assert_eq!(window.runner.lock().general_config.accel_config, applied);
        assert_eq!(window.config_revision(), 0);
    }

    #[test]
    fn recording_prevents_device_switch_even_without_drawing_ui() {
        let (mut window, _runtime) = window();
        window.choices.push(DeviceChoice {
            label: "Simulator".into(),
            connection: None,
        });
        window.runner.lock().record_packets = true;
        window.select_device(0);
        assert_eq!(window.selected, None);
        assert_eq!(window.device_revision(), 0);
        assert!(!window.busy());
    }

    #[test]
    fn worker_panic_releases_busy_state_without_a_completion_event() {
        let (mut window, runtime) = window();
        window.begin_operation(Operation::Reload);
        let (started, received) = tokio::sync::oneshot::channel();
        window.task = Some(runtime.spawn(async move {
            started.send(()).unwrap();
            panic!("simulated configuration worker panic");
        }));
        runtime.block_on(received).unwrap();
        window.poll_events();
        assert!(!window.busy());
        assert!(matches!(
            window.notice,
            Some(Notice {
                kind: NoticeKind::Error,
                ..
            })
        ));
    }

    #[test]
    fn zero_accelerometer_rate_is_rejected_before_apply() {
        let general = GeneralSettingsForm {
            accel_config: AccelConfig {
                accel_odr: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut errors = Vec::new();
        general.validate(&mut errors);
        assert!(errors.iter().any(|error| error.contains("data rate")));
    }
}
