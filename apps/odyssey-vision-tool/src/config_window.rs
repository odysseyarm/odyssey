mod pag_sensor_settings;
mod paj_sensor_settings;

use std::fs::File;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use eframe::egui;
use nusb::DeviceInfo;
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
/// A mux with no devices is kept in the list under this address so the next
/// refresh can reuse its connection.
const MUX_PLACEHOLDER_ADDR: [u8; 6] = [0; 6];

/// The configuration window: a device picker, one tab per settings group, and
/// the actions that write the settings to the device.
///
/// Device IO runs on the tokio runtime; results come back over a channel that
/// [`ConfigWindow::show`] drains at the top of every frame.
pub struct ConfigWindow {
    runtime: tokio::runtime::Handle,
    runner: Arc<Mutex<MotRunner>>,
    sender: EventSender,
    events: mpsc::Receiver<Event>,

    simulator_addr: Option<String>,
    udp_addr: Option<String>,
    /// Every connection the last scan found, including mux placeholders.
    connections: Vec<VmConnectionInfo>,
    /// What the picker lists: `connections` without placeholders, then the
    /// simulator and M4Hub entries.
    choices: Vec<DeviceChoice>,
    selected: Option<usize>,
    /// Identifies the latest connection attempt so that a slow, superseded
    /// attempt cannot install its device.
    connection_attempt: u64,
    connecting: bool,
    device: Option<VmDevice>,
    device_revision: u64,

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
        let mut window = Self {
            runtime,
            runner,
            sender: EventSender { tx, ctx },
            events,
            simulator_addr,
            udp_addr,
            connections: Vec::new(),
            choices: Vec::new(),
            selected: None,
            connection_attempt: 0,
            connecting: false,
            device: None,
            device_revision: 0,
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

    /// The accelerometer configuration as currently shown in the form.
    pub fn accel_config(&self) -> AccelConfig {
        self.general.accel_config
    }

    /// Applies pending async results, then draws the window.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.drain_events();
        self.expire_flash(ui.ctx());

        egui::Panel::top("config_device_bar").show(ui, |ui| {
            self.device_bar(ui);
            self.notice_banner(ui);
        });
        egui::Panel::bottom("config_actions").show(ui, |ui| self.action_bar(ui));
        egui::CentralPanel::default().show(ui, |ui| self.tabs(ui));
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.handle(event);
        }
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Devices(connections) => {
                self.connections = connections;
                self.rebuild_choices();
            }
            Event::Connected {
                attempt,
                device,
                settings,
            } => {
                if attempt != self.connection_attempt {
                    return;
                }
                self.connecting = false;
                self.install(*settings);
                self.set_device(Some(device));
            }
            Event::ConnectFailed { attempt, error } => {
                if attempt != self.connection_attempt {
                    return;
                }
                self.connecting = false;
                self.notify(Notice::error("Failed to connect", format!("{error:#}")));
            }
            Event::Reloaded(settings) => self.install(*settings),
            Event::Uploaded { subject, upload } => {
                match *upload {
                    Upload::Accel(config) => self.general.accel_config = config,
                    Upload::Gyro(config) => self.general.gyro_config = config,
                    Upload::NfIntrinsics(intrinsics) => self.general.nf_intrinsics = intrinsics,
                    Upload::WfIntrinsics(intrinsics) => self.general.wf_intrinsics = intrinsics,
                    Upload::StereoIso(iso) => self.general.stereo_iso = iso,
                }
                let noun = subject.noun();
                self.notify(Notice::info(
                    format!("Uploaded {noun}"),
                    format!("Successfully uploaded {noun}"),
                ));
            }
            Event::Flash(flash) => self.flash = Some((flash, Instant::now() + FLASH_DURATION)),
            Event::Notice(notice) => self.notify(notice),
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
        self.choices.clear();
        for connection in &self.connections {
            // Skip placeholder mux entries (used to keep connection alive when 0 devices)
            if let VmConnectionInfo::ViaMux { device_addr, .. } = connection
                && *device_addr == MUX_PLACEHOLDER_ADDR
            {
                continue;
            }
            self.choices.push(DeviceChoice {
                label: display_for_vm_connection(connection),
                connection: Some(connection.clone()),
            });
        }
        // The simulator and M4Hub entries have no connection path any more.
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
        if self
            .selected
            .is_some_and(|index| index >= self.choices.len())
        {
            self.selected = None;
        }
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

    // ---- drawing ----

    fn device_bar(&mut self, ui: &mut egui::Ui) {
        let mut picked = None;
        ui.horizontal(|ui| {
            ui.label("Device");
            let selected_text = self
                .selected
                .and_then(|index| self.choices.get(index))
                .map_or("Select a device", |choice| choice.label.as_str());
            egui::ComboBox::from_id_salt("config_device")
                .width(360.0)
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    for (index, choice) in self.choices.iter().enumerate() {
                        let is_selected = self.selected == Some(index);
                        if ui.selectable_label(is_selected, &choice.label).clicked() {
                            // Picking the shown entry again retries a failed connection.
                            if !is_selected || (self.device.is_none() && !self.connecting) {
                                picked = Some(index);
                            }
                        }
                    }
                });
            if ui.button("Refresh").clicked() {
                self.refresh_devices();
            }
            if self.connecting {
                ui.spinner();
                ui.label("Connecting…");
            }
        });
        if let Some(index) = picked {
            self.select_device(index);
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
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.device.is_some(), |ui| {
                if ui.button(apply_label).clicked() {
                    self.commit(Commit::Apply);
                }
                if ui.button(save_label).clicked() {
                    self.commit(Commit::Save);
                }
                if ui.button("Reload").clicked() {
                    self.reload();
                }
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
            .show(ui, |ui| match self.tab {
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

        if let Some(action) = general_action {
            self.general_action(action);
        }
        if let PagAction::CaptureImage = pag_action
            && let Some(device) = self.device.clone()
        {
            self.runtime.spawn(async move {
                PagSensorSettingsForm::capture_image(&device).await;
            });
        }
    }

    // ---- device connection ----

    fn refresh_devices(&self) {
        tracing::debug!("=== refresh_device_list called ===");
        let existing_mux = ExistingMux::find(&self.connections);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            match scan_devices(existing_mux).await {
                Ok(connections) => sender.send(Event::Devices(connections)),
                Err(e) => sender.send(Event::Notice(Notice::error(
                    "Failed to list usb devices",
                    e.to_string(),
                ))),
            }
        });
    }

    fn select_device(&mut self, index: usize) {
        self.selected = Some(index);
        self.general.clear();
        self.wf.clear();
        self.nf.clear();
        self.pag.clear();
        let old_device = self.set_device(None);
        self.connection_attempt += 1;
        self.connecting = true;

        let attempt = self.connection_attempt;
        let target = self
            .choices
            .get(index)
            .and_then(|choice| choice.connection.clone());
        let runner = Arc::clone(&self.runner);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            let event = match connect(old_device, target, &runner).await {
                Ok((device, settings)) => Event::Connected {
                    attempt,
                    device,
                    settings: Box::new(settings),
                },
                Err(error) => Event::ConnectFailed { attempt, error },
            };
            sender.send(event);
        });
    }

    // ---- toolbar actions ----

    /// Validates the forms, writes them to the device, and for
    /// [`Commit::Save`] also flashes them.
    fn commit(&mut self, commit: Commit) {
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
        let runner = Arc::clone(&self.runner);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            if let Err(notice) =
                Box::pin(apply_settings(&device, &general, &sensors, &runner)).await
            {
                sender.send(Event::Notice(notice));
                return;
            }
            match commit {
                Commit::Apply => sender.send(Event::Flash(Flash::Applied)),
                Commit::Save => match device.flash_settings().await {
                    Ok(()) => sender.send(Event::Flash(Flash::Saved)),
                    Err(e) => sender.send(Event::Notice(Notice::error(
                        "Failed to request flash settings",
                        e.to_string(),
                    ))),
                },
            }
        });
    }

    fn reload(&self) {
        let Some(device) = self.device.clone() else {
            return;
        };
        let runner = Arc::clone(&self.runner);
        let sender = self.sender.clone();
        self.runtime.spawn(async move {
            match load_settings(&device, &runner, LoadKind::Reload).await {
                Ok(settings) => sender.send(Event::Reloaded(Box::new(settings))),
                Err(e) => sender.send(Event::Notice(Notice::error(
                    "Failed to reload settings",
                    format!("{e:#}"),
                ))),
            }
        });
    }

    fn load_defaults(&mut self) {
        self.general.load_defaults();
        self.nf.load_defaults();
        self.wf.load_defaults();
        self.pag.load_defaults();
    }

    // ---- calibration and configuration files ----

    fn general_action(&mut self, action: GeneralAction) {
        match action {
            GeneralAction::Upload(document) => self.upload(document),
            GeneralAction::Download(document) => self.download(document),
            GeneralAction::SyncStereo => {
                self.general.stereo_iso = self.runner.lock().general_config.stereo_iso;
            }
        }
    }

    fn upload(&self, document: Document) {
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

    fn download(&self, document: Document) {
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

    /// Asks for a file on the blocking pool (the dialog blocks), parses it and
    /// reports the result as an event. Cancelling the dialog does nothing.
    fn upload_file(
        &self,
        subject: Subject,
        read: impl FnOnce(File) -> Result<Upload> + Send + 'static,
    ) {
        let sender = self.sender.clone();
        self.runtime.spawn_blocking(move || {
            let Some(path) = rfd::FileDialog::new().pick_file() else {
                return;
            };
            let result = File::open(&path)
                .map_err(anyhow::Error::from)
                .and_then(read);
            sender.send(match result {
                Ok(upload) => Event::Uploaded {
                    subject,
                    upload: Box::new(upload),
                },
                Err(e) => Event::Notice(Notice::error(
                    format!("Failed to upload {}", subject.noun()),
                    format!("Failed to read {}: {e:#}", path.display()),
                )),
            });
        });
    }

    /// Asks for a destination on the blocking pool and writes it. Cancelling
    /// the dialog does nothing.
    fn download_file(
        &self,
        subject: Subject,
        write: impl FnOnce(File) -> Result<()> + Send + 'static,
    ) {
        let sender = self.sender.clone();
        self.runtime.spawn_blocking(move || {
            let Some(path) = rfd::FileDialog::new().save_file() else {
                return;
            };
            let noun = subject.noun();
            let result = File::create(&path)
                .map_err(anyhow::Error::from)
                .and_then(write);
            sender.send(Event::Notice(match result {
                Ok(()) => Notice::info(
                    format!("Downloaded {noun}"),
                    format!("Successfully downloaded {noun}"),
                ),
                Err(e) => Notice::error(
                    format!("Failed to download {noun}"),
                    format!("Failed to write {}: {e:#}", path.display()),
                ),
            }));
        });
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

/// A dismissable message at the top of the window; replaces the modal dialogs.
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

/// Results of background work, applied by [`ConfigWindow::show`].
enum Event {
    Devices(Vec<VmConnectionInfo>),
    Connected {
        attempt: u64,
        device: VmDevice,
        settings: Box<LoadedSettings>,
    },
    ConnectFailed {
        attempt: u64,
        error: anyhow::Error,
    },
    Reloaded(Box<LoadedSettings>),
    Uploaded {
        subject: Subject,
        upload: Box<Upload>,
    },
    Flash(Flash),
    Notice(Notice),
}

/// Posts events to the window and wakes the UI up to show them.
#[derive(Clone)]
struct EventSender {
    tx: mpsc::Sender<Event>,
    ctx: egui::Context,
}

impl EventSender {
    fn send(&self, event: Event) {
        // The receiver is gone only when the window was dropped.
        if self.tx.send(event).is_ok() {
            self.ctx.request_repaint();
        }
    }
}

/// Everything read from a device.
struct LoadedSettings {
    general: GeneralSettingsForm,
    sensors: SensorSettings,
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

    /// Make sure to call `validate()` before calling this method.
    async fn apply(&self, device: &VmDevice) -> Result<(), Notice> {
        match self {
            Self::Vm { wf, nf } => {
                Box::pin(wf.apply(device)).await.map_err(|e| {
                    Notice::error("Failed to apply wide field settings", e.to_string())
                })?;
                Box::pin(nf.apply(device)).await.map_err(|e| {
                    Notice::error("Failed to apply near field settings", e.to_string())
                })
            }
            Self::Pro(pag) => Box::pin(pag.apply(device))
                .await
                .map_err(|e| Notice::error("Failed to apply PAG settings", e.to_string())),
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
    runner: &Mutex<MotRunner>,
) -> Result<(), Notice> {
    Box::pin(sensors.apply(device)).await?;
    general
        .apply(device, runner)
        .await
        .map_err(|e| Notice::error("Failed to apply general settings", e.to_string()))
}

/// Whether the tracker's own copy of the config is replaced by the load.
#[derive(Clone, Copy)]
enum LoadKind {
    /// First load after connecting: the tracker adopts the device's config.
    Initial,
    Reload,
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

    async fn load_from_device(
        device: &VmDevice,
        runner: &Mutex<MotRunner>,
        kind: LoadKind,
    ) -> Result<Self> {
        let timeout = Duration::from_millis(5000);

        // Read all config values at once
        let config = match retry(|| device.read_all_config(), timeout, 3).await {
            Some(Ok(cfg)) => cfg,
            Some(Err(e)) => return Err(e),
            None => {
                return Err(anyhow::anyhow!(
                    "Timeout reading device config after 3 attempts"
                ));
            }
        };

        // Read individual prop fields
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

        // Extract values from enum variants
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

        if let LoadKind::Initial = kind {
            runner.lock().general_config = config;
        }
        Ok(form)
    }

    fn validate(&self, errors: &mut Vec<String>) {
        if !(0..256).contains(&self.impact_threshold) {
            errors.push("impact threshold: must be between 0 and 255".into());
        }
        if !(0..256).contains(&self.suppress_ms) {
            errors.push("suppress (ms): must be between 0 and 255".into());
        }
    }

    /// Make sure to call `validate()` before calling this method.
    async fn apply(&self, device: &VmDevice, runner: &Mutex<MotRunner>) -> Result<()> {
        let config = GeneralSettings {
            impact_threshold: register_value(self.impact_threshold, "impact threshold")?,
            suppress_ms: register_value(self.suppress_ms, "suppress (ms)")?,
            accel_config: self.accel_config,
            gyro_config: self.gyro_config,
            camera_model_nf: self.nf_intrinsics.clone(),
            camera_model_wf: self.wf_intrinsics.clone(),
            stereo_iso: self.stereo_iso,
        };

        // Write each config field separately using the new API
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

        {
            let general_config = &mut runner.lock().general_config;
            general_config.impact_threshold = config.impact_threshold;
            general_config.suppress_ms = config.suppress_ms;
            general_config.accel_config = config.accel_config;
            general_config.gyro_config = config.gyro_config;
            general_config.camera_model_nf = config.camera_model_nf;
            general_config.camera_model_wf = config.camera_model_wf;
            general_config.stereo_iso = config.stereo_iso;
        }
        Ok(())
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

        // Set intrinsics based on device type
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
    runner: &Mutex<MotRunner>,
) -> Result<(VmDevice, LoadedSettings)> {
    // Clean up old device if it exists, BEFORE connecting to new one
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
    // Disable all streams from previous session to prevent flooding
    if let Err(e) = device.clear_all_streams().await {
        tracing::warn!("Failed to disable streams: {e:?}");
    }
    tracing::debug!("Streams disabled, loading general settings...");

    let settings = load_settings(&device, runner, LoadKind::Initial).await?;
    tracing::debug!("All settings loaded, setting device...");
    Ok((device, settings))
}

async fn load_settings(
    device: &VmDevice,
    runner: &Mutex<MotRunner>,
    kind: LoadKind,
) -> Result<LoadedSettings> {
    let general = GeneralSettingsForm::load_from_device(device, runner, kind)
        .await
        .inspect_err(|e| tracing::error!("Failed to load general settings: {e:?}"))?;
    tracing::debug!("General settings loaded successfully");
    let sensors = load_sensor_settings(device, general.device_pid).await?;
    Ok(LoadedSettings { general, sensors })
}

/// A mux connection from the previous scan, reused so that it does not
/// disrupt devices that are connected through it.
struct ExistingMux {
    mux: MuxDevice,
    device_addrs: Vec<[u8; 6]>,
}

impl ExistingMux {
    fn find(connections: &[VmConnectionInfo]) -> Option<Self> {
        let mut mux = None;
        let mut device_addrs = Vec::new();
        for connection in connections {
            if let VmConnectionInfo::ViaMux {
                mux: found,
                device_addr,
            } = connection
            {
                if mux.is_none() {
                    mux = Some(found.clone());
                }
                device_addrs.push(*device_addr);
            }
        }
        mux.map(|mux| Self { mux, device_addrs })
    }
}

/// Lists the connectable devices: vision modules on USB, and the vision
/// modules behind every mux.
async fn scan_devices(existing_mux: Option<ExistingMux>) -> Result<Vec<VmConnectionInfo>> {
    let usb_devices: Vec<DeviceInfo> = nusb::list_devices()
        .await?
        .filter(|info| info.vendor_id() == VENDOR_ID)
        .collect();

    // Separate direct USB vision modules and mux devices, probing transport mode for vision modules
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

    // No muxes found, just use direct USB connections
    if hub_devices.is_empty() {
        return Ok(vm_connections);
    }

    let mut all_connections = vm_connections;
    if let Some(existing) = existing_mux {
        tracing::debug!(
            "Reusing existing mux connection for refresh (with {} existing mux devices)",
            existing.device_addrs.len()
        );
        scan_existing_mux(existing, &mut all_connections).await;
    } else {
        tracing::debug!("No existing mux connection, creating new one");
        // Query each mux for connected devices
        for hub_info in hub_devices {
            scan_new_mux(hub_info, &mut all_connections).await;
        }
    }
    tracing::debug!(
        "Updating device list with {} total devices",
        all_connections.len()
    );
    Ok(all_connections)
}

async fn scan_existing_mux(existing: ExistingMux, connections: &mut Vec<VmConnectionInfo>) {
    let ExistingMux { mux, device_addrs } = existing;
    tracing::debug!("Querying devices from existing mux...");
    let device_addrs = match mux.request_devices().await {
        Ok(devices) => {
            tracing::debug!("Mux query successful, found {} device(s)", devices.len());
            devices.into_iter().collect()
        }
        Err(e) => {
            tracing::warn!("Failed to query existing mux: {e}");
            tracing::debug!(
                "Preserving {} existing mux devices in the list",
                device_addrs.len()
            );
            device_addrs
        }
    };
    for device_addr in device_addrs {
        tracing::debug!("  Device: {}", format_device_addr(device_addr));
        connections.push(VmConnectionInfo::ViaMux {
            mux: mux.clone(),
            device_addr,
        });
    }
}

async fn scan_new_mux(hub_info: DeviceInfo, connections: &mut Vec<VmConnectionInfo>) {
    tracing::debug!("Attempting to connect to mux...");
    let hub = match MuxDevice::connect_usb(hub_info).await {
        Ok(hub) => hub,
        Err(e) => {
            tracing::warn!("Failed to connect to mux: {e}");
            return;
        }
    };
    tracing::debug!("Mux connected successfully, requesting devices...");
    match hub.request_devices().await {
        Ok(devices) => {
            tracing::debug!("Mux query successful, found {} device(s)", devices.len());
            // If no devices, add a placeholder to keep mux alive for next refresh
            if devices.is_empty() {
                tracing::debug!(
                    "No devices found, adding placeholder to keep mux connection alive"
                );
                connections.push(VmConnectionInfo::ViaMux {
                    mux: hub,
                    device_addr: MUX_PLACEHOLDER_ADDR,
                });
            } else {
                for device_addr in devices {
                    tracing::debug!("  Device: {}", format_device_addr(device_addr));
                    connections.push(VmConnectionInfo::ViaMux {
                        mux: hub.clone(),
                        device_addr,
                    });
                }
            }
        }
        Err(e) => {
            tracing::warn!("Failed to query mux devices: {e}");
        }
    }
    tracing::debug!("Mux devices added to connection list");
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
            let pag = PagSensorSettingsForm::load_from_device(device)
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
