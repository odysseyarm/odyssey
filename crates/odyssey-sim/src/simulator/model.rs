use std::collections::BTreeMap;

use odyssey_protocol::control::device::{self, DeviceMsg, TransportMode};
use odyssey_protocol::control::usb_mux::{self, UsbMuxCtrlMsg};
use odyssey_protocol::control::{AddBondError, AddBondResult, BondEntry, BondedDevice};
use odyssey_protocol::image::{IMAGE_LEASE_TIMEOUT_MS, ImageCommand, ImageStatus, ImageStatusCode};
use odyssey_protocol::mux::{self, DevicePacket, MuxMsg};
use odyssey_protocol::{
    AccelReport, BatteryReport, ConfigKind, DeviceName, GeneralConfig, ImpactReport, Packet,
    PacketData, PacketType, ProductId, PropKind, Props, ReadRegisterResponse, SetDeviceNameResult,
    StreamUpdateAction, Version,
};
use tokio::sync::{broadcast, mpsc};

use super::image_stream::ImageSession;
use super::{Command, DONGLE_UUID, SensorModel, SimError, Snapshot, Transport, UNIT_UUID, scene};

type Registers = BTreeMap<(u8, u8), u8>;

fn reset_registers(sensor_model: SensorModel) -> Registers {
    if sensor_model == SensorModel::Paj7025 {
        // Valid PAJ object-mode configuration before entering image mode.
        return [
            ((0x00, 0x02), 0x25),
            ((0x00, 0x03), 0x70),
            ((0x0c, 0x60), 0xfe),
            ((0x0c, 0x61), 0x0f),
            ((0x0c, 0x62), 0xfe),
            ((0x0c, 0x63), 0x0f),
            ((0x01, 0x0e), 0xe8),
            ((0x01, 0x0f), 0x03),
            ((0x0c, 0x07), 0xa0),
            ((0x0c, 0x08), 0x86),
            ((0x0c, 0x09), 0x01),
            ((0x00, 0x19), 16),
            ((0x00, 0x0b), 0xf4),
            ((0x00, 0x0c), 0x01),
            ((0x0c, 0x46), 10),
            ((0x0c, 0x47), 128),
        ]
        .into_iter()
        .collect();
    }
    // Bank-zero PAG settings loaded by Lite firmware at startup.
    [
        (0x67, 0x94),
        (0x68, 3),
        (0x6d, 120),
        (0x6e, 10),
        (0x6f, 0),
        (0x70, 0xf4),
        (0x71, 1),
        (0x23, 0x1a),
        (0x24, 0x26),
        (0x77, 0x17),
        (0x78, 0x1d),
        (0x13, 180),
    ]
    .into_iter()
    .map(|(address, value)| ((0, address), value))
    .collect()
}

pub(crate) struct Model {
    pub(crate) state: Snapshot,
    pub(crate) usb: Option<mpsc::Sender<Packet>>,
    pub(crate) unit_control: Option<mpsc::Sender<DeviceMsg>>,
    pub(crate) mux: Option<mpsc::Sender<MuxMsg>>,
    pub(crate) mux_control: Option<mpsc::Sender<UsbMuxCtrlMsg>>,
    configs: [GeneralConfig; 7],
    persisted: [GeneralConfig; 7],
    registers: Registers,
    persisted_registers: Registers,
    unit_bond: Option<BondEntry>,
    dongle_bonds: BTreeMap<[u8; 6], BondEntry>,
    bond_names: BTreeMap<[u8; 6], DeviceName>,
    unit_deadline: Option<u64>,
    dongle_deadline: Option<u64>,
    ble_resume: u64,
    list_subscribed: bool,
    streams: [Option<u8>; 256],
    last_shot: Option<u64>,
    last_imu: u64,
    last_markers: u64,
    last_battery: Option<u64>,
    reboot_at: Option<u64>,
    images: Option<ImageSession>,
    sensor_model: SensorModel,
    image_mode_generation: Option<u32>,
    orientation: nalgebra::UnitQuaternion<f32>,
    errors: broadcast::Sender<SimError>,
}

impl Model {
    pub(crate) fn new(errors: broadcast::Sender<SimError>, sensor_model: SensorModel) -> Self {
        let camera: odyssey_protocol::wire::CameraCalibrationParams =
            odyssey_protocol::wire::CameraCalibrationParams {
                camera_matrix: [500.0, 0.0, 320.0, 0.0, 500.0, 240.0, 0.0, 0.0, 1.0],
                dist_coeffs: [0.0; 5],
            };
        let configs = [
            GeneralConfig::ImpactThreshold(100),
            GeneralConfig::SuppressMs(100),
            GeneralConfig::AccelConfig(Default::default()),
            GeneralConfig::GyroConfig(Default::default()),
            GeneralConfig::CameraModelNf(camera.clone().into()),
            GeneralConfig::CameraModelWf(camera.into()),
            GeneralConfig::StereoIso(Default::default()),
        ];
        Self {
            state: Snapshot::default(),
            usb: None,
            unit_control: None,
            mux: None,
            mux_control: None,
            persisted: configs.clone(),
            configs,
            registers: reset_registers(sensor_model),
            persisted_registers: reset_registers(sensor_model),
            unit_bond: None,
            dongle_bonds: BTreeMap::new(),
            bond_names: BTreeMap::new(),
            unit_deadline: None,
            dongle_deadline: None,
            ble_resume: 0,
            list_subscribed: false,
            streams: [None; 256],
            last_shot: None,
            last_imu: 0,
            last_markers: 0,
            last_battery: None,
            reboot_at: None,
            images: None,
            sensor_model,
            image_mode_generation: None,
            errors,
            orientation: nalgebra::UnitQuaternion::identity(),
        }
    }

    pub(crate) fn error(&mut self, error: SimError) {
        self.state.last_error = Some(error.to_string());
        let _ = self.errors.send(error);
    }

    fn unsupported(&mut self, request: impl std::fmt::Debug) {
        self.error(SimError::Unsupported(format!("{request:?}")));
    }

    pub(crate) fn disconnect_usb(&mut self) {
        self.usb = None;
        self.unit_control = None;
        self.images = None;
    }

    pub(crate) fn disconnect_mux(&mut self) {
        self.mux = None;
        self.mux_control = None;
        self.list_subscribed = false;
    }

    fn send<T>(sender: Option<&mpsc::Sender<T>>, message: T) -> Result<(), SimError> {
        match sender {
            Some(sender) => sender.try_send(message).map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => {
                    SimError::Backpressure("host stopped reading")
                }
                mpsc::error::TrySendError::Closed(_) => SimError::Stopped,
            }),
            None => Ok(()),
        }
    }

    fn unit_event(&mut self, event: DeviceMsg) {
        if let Err(error) = Self::send(self.unit_control.as_ref(), event) {
            self.error(error);
        }
    }

    fn mux_event(&mut self, event: UsbMuxCtrlMsg) {
        if let Err(error) = Self::send(self.mux_control.as_ref(), event) {
            self.error(error);
        }
    }

    pub(crate) fn list(&mut self) {
        let mut devices = odyssey_protocol::List::default();
        if self.state.ble_connected {
            let _ = devices.0.push(UNIT_UUID);
        }
        if let Err(error) = Self::send(self.mux.as_ref(), MuxMsg::DevicesSnapshot(devices)) {
            self.error(error);
        }
    }

    fn matches_bonds(&self) -> bool {
        match (self.unit_bond.as_ref(), self.dongle_bonds.get(&UNIT_UUID)) {
            (Some(unit), Some(dongle)) => {
                unit.is_bonded
                    && dongle.is_bonded
                    && unit.bd_addr == DONGLE_UUID
                    && unit.ltk == dongle.ltk
            }
            _ => false,
        }
    }

    pub(crate) fn reconcile(&mut self) {
        if self.unit_deadline.is_some()
            && self.dongle_deadline.is_some()
            && self.state.ble_in_range
            && self.state.dongle_usb
        {
            if self.dongle_bonds.len() >= mux::MAX_DEVICES
                && !self.dongle_bonds.contains_key(&UNIT_UUID)
            {
                self.dongle_deadline = None;
                self.mux_event(UsbMuxCtrlMsg::BondStoreError(usb_mux::BondStoreError::Full));
            } else {
                let ltk = [0x53; 16];
                self.unit_bond = Some(BondEntry {
                    bd_addr: DONGLE_UUID,
                    ltk,
                    security_level: 2,
                    is_bonded: true,
                    irk: None,
                });
                self.dongle_bonds.insert(
                    UNIT_UUID,
                    BondEntry {
                        bd_addr: UNIT_UUID,
                        ltk,
                        security_level: 2,
                        is_bonded: true,
                        irk: None,
                    },
                );
                let name = DeviceName::from_str_lossy(&self.state.name);
                self.bond_names.insert(UNIT_UUID, name.clone());
                self.unit_deadline = None;
                self.dongle_deadline = None;
                self.unit_event(DeviceMsg::PairingResult(device::PairingOutcome::Ok(
                    BondedDevice {
                        uuid: DONGLE_UUID,
                        name: DeviceName::from_str_lossy("Odyssey Simulated Dongle"),
                    },
                )));
                self.mux_event(UsbMuxCtrlMsg::PairingResult(usb_mux::PairingOutcome::Ok(
                    BondedDevice {
                        uuid: UNIT_UUID,
                        name,
                    },
                )));
            }
        }
        self.state.bonded = self.matches_bonds();
        let connected = self.state.bonded
            && self.state.ble_in_range
            && self.state.dongle_usb
            && self.state.time_ms >= self.ble_resume;
        let changed = connected != self.state.ble_connected;
        self.state.ble_connected = connected;
        self.state.unit_pairing = self.unit_deadline.is_some();
        self.state.dongle_pairing = self.dongle_deadline.is_some();
        self.state.transport = if self.state.unit_usb {
            Transport::Usb
        } else {
            Transport::Ble
        };
        if changed && self.list_subscribed {
            self.list();
        }
    }

    fn persist(&mut self) {
        self.persisted = self.configs.clone();
        self.persisted_registers = self.registers.clone();
    }

    fn write_config(&mut self, config: GeneralConfig) {
        let index = match &config {
            GeneralConfig::ImpactThreshold(_) => 0,
            GeneralConfig::SuppressMs(_) => 1,
            GeneralConfig::AccelConfig(_) => 2,
            GeneralConfig::GyroConfig(_) => 3,
            GeneralConfig::CameraModelNf(_) => 4,
            GeneralConfig::CameraModelWf(_) => 5,
            GeneralConfig::StereoIso(_) => 6,
        };
        self.configs[index] = config;
    }

    fn prop(&self, kind: PropKind) -> Props {
        match kind {
            PropKind::Uuid => Props::Uuid(UNIT_UUID),
            PropKind::ProductId => Props::ProductId(match self.sensor_model {
                SensorModel::Pag7665qn => ProductId::AtsPro,
                SensorModel::Paj7025 => ProductId::AtsLite,
            } as u16),
            PropKind::Name => Props::Name(DeviceName::from_str_lossy(&self.state.name)),
            PropKind::Version => Props::Version(Version::new([0, 0, 0])),
            // Zero means no net-core firmware release, not a fabricated real release.
            PropKind::NetCoreRelease => Props::NetCoreRelease(0),
        }
    }

    pub(crate) fn packet(&mut self, packet: Packet, ble: bool) {
        if ble && !self.state.ble_connected || !ble && !self.state.unit_usb {
            return;
        }
        let id = packet.id;
        let response = match packet.data {
            PacketData::ReadVersion() => {
                Some(PacketData::ReadVersionResponse(Version::new([0, 0, 0])))
            }
            PacketData::ReadProp(kind) => Some(PacketData::ReadPropResponse(self.prop(kind))),
            PacketData::ReadConfig(kind) => Some(PacketData::ReadConfigResponse(
                self.configs[kind as usize].clone(),
            )),
            PacketData::WriteConfig(config) => {
                self.write_config(config);
                Some(PacketData::Ack())
            }
            PacketData::FlashSettings() => {
                self.persist();
                Some(PacketData::Ack())
            }
            PacketData::WriteRegister(register) => {
                // Lite has one PAG sensor; firmware ignores the requested port.
                self.registers
                    .insert((register.bank, register.address), register.data);
                Some(PacketData::Ack())
            }
            PacketData::ReadRegister(register) => {
                let data = self
                    .registers
                    .get(&(register.bank, register.address))
                    .copied()
                    .unwrap_or(0);
                Some(PacketData::ReadRegisterResponse(ReadRegisterResponse {
                    bank: register.bank,
                    address: register.address,
                    data,
                }))
            }
            PacketData::SetDeviceName(name) => {
                self.state.name = name.as_str().to_owned();
                if ble {
                    self.bond_names.insert(UNIT_UUID, name);
                }
                Some(PacketData::SetDeviceNameResponse(SetDeviceNameResult::Ok()))
            }
            PacketData::ImageCommand(command) => self
                .image_command(id, command, ble)
                .map(PacketData::ImageStatus),
            PacketData::WriteMode(odyssey_protocol::Mode::Image) => {
                Some(PacketData::ImageStatus(ImageStatus {
                    generation: 0,
                    status: ImageStatusCode::Unsupported,
                }))
            }
            PacketData::WriteMode(odyssey_protocol::Mode::Object) => Some(PacketData::Ack()),
            PacketData::StreamUpdate(update) => {
                let index = u8::from(update.packet_id) as usize;
                match update.action {
                    StreamUpdateAction::DisableAll => self.streams.fill(None),
                    StreamUpdateAction::Disable => self.streams[index] = None,
                    StreamUpdateAction::Enable
                        if matches!(
                            update.packet_id,
                            PacketType::PocMarkersReport()
                                | PacketType::AccelReport()
                                | PacketType::ImpactReport()
                                | PacketType::BatteryReport()
                        ) =>
                    {
                        self.streams[index] = Some(id);
                        if matches!(update.packet_id, PacketType::BatteryReport()) {
                            self.last_battery = None;
                        }
                    }
                    StreamUpdateAction::Enable => {
                        self.unsupported(update);
                        return;
                    }
                }
                Some(PacketData::Ack())
            }
            data => {
                self.unsupported(data);
                None
            }
        };
        if id == 255 {
            return;
        }
        if let Some(data) = response {
            let packet = Packet { id, data };
            let result = if ble {
                Self::send(
                    self.mux.as_ref(),
                    MuxMsg::DevicePacket(DevicePacket {
                        dev: UNIT_UUID,
                        pkt: packet,
                    }),
                )
            } else {
                Self::send(self.usb.as_ref(), packet)
            };
            if let Err(error) = result {
                self.error(error);
            }
        }
    }

    /// `None` for Renew, which firmware never answers.
    fn image_command(
        &mut self,
        request_id: u8,
        command: ImageCommand,
        ble: bool,
    ) -> Option<ImageStatus> {
        let (generation, status) = match command {
            ImageCommand::Renew { generation } => {
                if !ble
                    && let Some(session) = &mut self.images
                    && session.generation == generation
                {
                    session.lease_until_ms =
                        self.state.time_ms.saturating_add(IMAGE_LEASE_TIMEOUT_MS);
                }
                return None;
            }
            ImageCommand::Start { generation, config } => {
                let status = if ble {
                    ImageStatusCode::UsbRequired
                } else if self.sensor_model == SensorModel::Pag7665qn
                    && config.camera != odyssey_protocol::Port::Nf
                {
                    ImageStatusCode::UnsupportedCamera
                } else if !(1..=15).contains(&config.fps) {
                    ImageStatusCode::CaptureError
                } else if self.images.is_some() {
                    ImageStatusCode::Busy
                } else if self.image_mode_generation.is_some() {
                    ImageStatusCode::RebootRequired
                } else {
                    if self.sensor_model == SensorModel::Paj7025 {
                        self.image_mode_generation = Some(generation);
                    }
                    self.images = Some(ImageSession::new(
                        generation,
                        request_id,
                        config,
                        self.sensor_model,
                        self.state.time_ms.saturating_add(IMAGE_LEASE_TIMEOUT_MS),
                    ));
                    ImageStatusCode::Started
                };
                (generation, status)
            }
            ImageCommand::Stop { generation } => {
                let status = if ble {
                    ImageStatusCode::UsbRequired
                } else {
                    if self
                        .images
                        .as_ref()
                        .is_some_and(|session| session.generation == generation)
                    {
                        self.images = None;
                    }
                    if self.image_mode_generation == Some(generation) {
                        ImageStatusCode::RebootRequired
                    } else {
                        ImageStatusCode::Stopped
                    }
                };
                (generation, status)
            }
        };
        Some(ImageStatus { generation, status })
    }

    pub(crate) fn mux_packet(&mut self, message: MuxMsg) {
        if !self.state.dongle_usb {
            return;
        }
        match message {
            MuxMsg::RequestDevices() => self.list(),
            MuxMsg::SubscribeDeviceList() => {
                self.list_subscribed = true;
                self.list();
            }
            MuxMsg::UnsubscribeDeviceList() => self.list_subscribed = false,
            MuxMsg::ReadVersion() => {
                if let Err(error) = Self::send(
                    self.mux.as_ref(),
                    MuxMsg::ReadVersionResponse(mux::Version::new([0, 0, 0])),
                ) {
                    self.error(error);
                }
            }
            MuxMsg::SendTo(request) if request.dev == UNIT_UUID && self.state.ble_connected => {
                self.packet(request.pkt, true);
            }
            other => self.unsupported(other),
        }
    }

    pub(crate) fn unit_control(&mut self, message: DeviceMsg) {
        if !self.state.unit_usb {
            return;
        }
        match message {
            DeviceMsg::ReadProp(kind) => {
                self.unit_event(DeviceMsg::ReadPropResponse(self.prop(kind)));
            }
            DeviceMsg::GetTransportMode() | DeviceMsg::SetTransportMode(_) => {
                self.unit_event(DeviceMsg::TransportModeStatus(if self.state.unit_usb {
                    TransportMode::Usb
                } else {
                    TransportMode::Ble
                }));
            }
            DeviceMsg::ReadConfig(kind) => self.unit_event(DeviceMsg::ReadConfigResponse(
                self.configs[kind as usize].clone(),
            )),
            DeviceMsg::WriteConfig(config) => {
                self.write_config(config);
                self.unit_event(DeviceMsg::WriteConfigAck());
            }
            DeviceMsg::FlashSettings() => {
                self.persist();
                self.unit_event(DeviceMsg::FlashSettingsAck());
            }
            DeviceMsg::SetDeviceName(name) => {
                self.state.name = name.as_str().to_owned();
                self.unit_event(DeviceMsg::SetDeviceNameResponse(SetDeviceNameResult::Ok()));
            }
            DeviceMsg::StartPairing(parameters) => {
                self.unit_deadline = Some(
                    self.state
                        .time_ms
                        .saturating_add(u64::from(parameters.timeout_ms)),
                );
                self.unit_event(DeviceMsg::StartPairingResponse());
            }
            DeviceMsg::CancelPairing() => self.cancel_unit(device::PairingError::Cancelled),
            DeviceMsg::ClearBond() => {
                self.unit_bond = None;
                self.ble_resume = self.state.time_ms.saturating_add(10);
                self.unit_event(DeviceMsg::ClearBondResponse(device::ClearBondResult::Ok()));
            }
            DeviceMsg::AddBond(entry) => {
                self.unit_bond = Some(entry);
                self.ble_resume = self.state.time_ms.saturating_add(10);
                self.unit_event(DeviceMsg::AddBondResponse(AddBondResult::Ok()));
            }
            DeviceMsg::Reboot() => {
                self.unit_event(DeviceMsg::RebootAck());
                self.configs = self.persisted.clone();
                self.registers = self.persisted_registers.clone();
                self.streams.fill(None);
                self.reboot_at = Some(self.state.time_ms.saturating_add(50));
            }
            other => self.unsupported(other),
        }
        self.reconcile();
    }

    fn cancel_unit(&mut self, error: device::PairingError) {
        if self.unit_deadline.take().is_some() {
            self.unit_event(DeviceMsg::PairingResult(device::PairingOutcome::Err(error)));
        }
    }

    fn cancel_dongle(&mut self, error: usb_mux::PairingError) {
        if self.dongle_deadline.take().is_some() {
            self.mux_event(UsbMuxCtrlMsg::PairingResult(usb_mux::PairingOutcome::Err(
                error,
            )));
        }
    }

    pub(crate) fn dongle_control(&mut self, message: UsbMuxCtrlMsg) {
        if !self.state.dongle_usb {
            return;
        }
        match message {
            UsbMuxCtrlMsg::ReadVersion() => {
                self.mux_event(UsbMuxCtrlMsg::ReadVersionResponse(Version::new([0, 0, 0])));
            }
            UsbMuxCtrlMsg::ListBonds() => {
                let mut bonds = odyssey_protocol::List(Default::default());
                for uuid in self.dongle_bonds.keys() {
                    let _ = bonds.0.push(BondedDevice {
                        uuid: *uuid,
                        name: self.bond_names.get(uuid).cloned().unwrap_or_default(),
                    });
                }
                self.mux_event(UsbMuxCtrlMsg::ListBondsResponse(bonds));
            }
            UsbMuxCtrlMsg::ClearBonds() => {
                self.dongle_deadline = None;
                self.dongle_bonds.clear();
                self.bond_names.clear();
                self.mux_event(UsbMuxCtrlMsg::ClearBondsResponse(
                    usb_mux::ClearBondsResult::Ok(),
                ));
            }
            UsbMuxCtrlMsg::StartPairing(parameters) => {
                self.dongle_deadline = Some(
                    self.state
                        .time_ms
                        .saturating_add(u64::from(parameters.timeout_ms)),
                );
                self.mux_event(UsbMuxCtrlMsg::StartPairingResponse());
            }
            UsbMuxCtrlMsg::CancelPairing() => self.cancel_dongle(usb_mux::PairingError::Cancelled),
            UsbMuxCtrlMsg::AddBond(entry) => {
                if self.dongle_bonds.len() >= mux::MAX_DEVICES
                    && !self.dongle_bonds.contains_key(&entry.bd_addr)
                {
                    self.mux_event(UsbMuxCtrlMsg::AddBondResponse(AddBondResult::Err(
                        AddBondError::Full,
                    )));
                } else {
                    self.dongle_bonds.insert(entry.bd_addr, entry);
                    self.mux_event(UsbMuxCtrlMsg::AddBondResponse(AddBondResult::Ok()));
                }
            }
            UsbMuxCtrlMsg::UpdateBondName { uuid, name } => {
                if self.dongle_bonds.contains_key(&uuid) {
                    self.bond_names.insert(uuid, name);
                }
            }
            other => self.unsupported(other),
        }
        self.reconcile();
    }

    pub(crate) fn command(&mut self, command: Command) -> Result<(), SimError> {
        match command {
            Command::UnitUsb { plugged } => {
                if self.state.unit_usb != plugged {
                    self.state.unit_usb_generation = self.state.unit_usb_generation.wrapping_add(1);
                }
                self.state.unit_usb = plugged;
                if !plugged {
                    self.disconnect_usb();
                }
            }
            Command::DongleUsb { plugged } => {
                if self.state.dongle_usb != plugged {
                    self.state.dongle_usb_generation =
                        self.state.dongle_usb_generation.wrapping_add(1);
                }
                self.state.dongle_usb = plugged;
                if !plugged {
                    self.disconnect_mux();
                    self.dongle_deadline = None;
                }
            }
            Command::BleRange { in_range } => self.state.ble_in_range = in_range,
            Command::Pair { timeout_ms } => {
                if timeout_ms == 0 {
                    return Err(SimError::InvalidInput("pairing timeout must be positive"));
                }
                let deadline = self.state.time_ms.saturating_add(u64::from(timeout_ms));
                self.unit_deadline = Some(deadline);
                self.dongle_deadline = Some(deadline);
            }
            Command::CancelPairing => {
                self.cancel_unit(device::PairingError::Cancelled);
                self.cancel_dongle(usb_mux::PairingError::Cancelled);
            }
            Command::ClearBonds => {
                self.unit_deadline = None;
                self.dongle_deadline = None;
                self.unit_bond = None;
                self.dongle_bonds.clear();
                self.bond_names.clear();
            }
            Command::Aim { x, y } => {
                if !x.is_finite()
                    || !y.is_finite()
                    || !(0.0..=1.0).contains(&x)
                    || !(0.0..=1.0).contains(&y)
                {
                    return Err(SimError::InvalidInput(
                        "aim must be finite and normalized to [0, 1]",
                    ));
                }
                self.state.aim = [x, y];
                self.orientation = nalgebra::UnitQuaternion::identity();
                self.emit_markers();
            }
            Command::Motion { accel, gyro } => {
                if accel.iter().chain(gyro.iter()).any(|v| !v.is_finite()) {
                    return Err(SimError::InvalidInput("motion must contain finite values"));
                }
                self.state.accel = accel;
                self.state.gyro = gyro;
                self.emit_imu();
            }
            Command::Shot => {
                let GeneralConfig::SuppressMs(suppress) = self.configs[1] else {
                    unreachable!()
                };
                if self.last_shot.is_none_or(|time| {
                    self.state.time_ms.saturating_sub(time) >= u64::from(suppress)
                }) {
                    self.last_shot = Some(self.state.time_ms);
                    self.state.shots = self.state.shots.wrapping_add(1);
                    self.report(PacketData::ImpactReport(ImpactReport {
                        timestamp: self.timestamp(),
                    }));
                }
            }
            Command::Battery { percent, charging } => {
                if percent > 100 {
                    return Err(SimError::InvalidInput(
                        "battery percent must not exceed 100",
                    ));
                }
                self.state.battery_percent = percent;
                self.state.charging = charging;
                self.emit_battery();
            }
            Command::Advance { millis } => {
                let Some(end) = self.state.time_ms.checked_add(millis) else {
                    return Err(SimError::InvalidInput("clock overflow"));
                };
                // Fixed substeps make scripted reporting and timeout order independent of command batching.
                while self.state.time_ms < end {
                    let next = self.state.time_ms.saturating_add(10).min(end);
                    self.tick(next);
                }
            }
        }
        self.reconcile();
        Ok(())
    }

    pub(crate) fn tick(&mut self, time: u64) {
        let dt = time.saturating_sub(self.state.time_ms) as f32 / 1000.0;
        let [x, y, z] = self.state.gyro;
        // Hub prediction uses the sensor's negated x/z/y ordering.
        self.orientation *=
            nalgebra::UnitQuaternion::from_scaled_axis(nalgebra::Vector3::new(-x, -z, -y) * dt);
        self.state.time_ms = time;
        if self.unit_deadline.is_some_and(|deadline| deadline <= time) {
            self.cancel_unit(device::PairingError::Timeout);
        }
        if self
            .dongle_deadline
            .is_some_and(|deadline| deadline <= time)
        {
            self.cancel_dongle(usb_mux::PairingError::Timeout);
        }
        if self.reboot_at.is_some_and(|deadline| deadline <= time) {
            self.reboot_at = None;
            self.image_mode_generation = None;
            // Reset closes the USB session; the manager observes a new generation and reopens it.
            self.disconnect_usb();
            self.state.unit_usb_generation = self.state.unit_usb_generation.wrapping_add(1);
        }
        self.reconcile();
        let GeneralConfig::AccelConfig(config) = &self.configs[2] else {
            unreachable!()
        };
        let interval = 1000 / u64::from(config.accel_odr.max(1));
        if time.saturating_sub(self.last_imu) >= interval.max(1) {
            self.emit_imu();
        }
        if time.saturating_sub(self.last_markers) >= 10 {
            self.emit_markers();
        }
        if self
            .last_battery
            .is_none_or(|last| time.saturating_sub(last) >= 1000)
        {
            self.emit_battery();
        }
        if self.sensor_model == SensorModel::Pag7665qn
            && let Some(images) = &self.images
            && time >= images.lease_until_ms
        {
            let expired = Packet {
                id: images.request_id,
                data: PacketData::ImageStatus(ImageStatus {
                    generation: images.generation,
                    status: ImageStatusCode::LeaseExpired,
                }),
            };
            self.images = None;
            if let Err(error) = Self::send(self.usb.as_ref(), expired) {
                self.error(error);
            }
        }
        if let Some(images) = &mut self.images
            && self
                .usb
                .as_ref()
                .is_none_or(|sender| !images.tick(time, sender))
        {
            self.images = None;
        }
    }

    fn timestamp(&self) -> u32 {
        self.state.time_ms.wrapping_mul(1000) as u32
    }

    fn report(&mut self, data: PacketData) {
        let packet = Packet { id: 0, data };
        let Some(id) = self.streams[u8::from(packet.ty()) as usize] else {
            return;
        };
        let packet = Packet { id, ..packet };
        let result = if self.state.unit_usb {
            Self::send(self.usb.as_ref(), packet)
        } else if self.state.ble_connected {
            Self::send(
                self.mux.as_ref(),
                MuxMsg::DevicePacket(DevicePacket {
                    dev: UNIT_UUID,
                    pkt: packet,
                }),
            )
        } else {
            return;
        };
        if let Err(error) = result {
            self.error(error);
        }
    }

    fn emit_imu(&mut self) {
        self.last_imu = self.state.time_ms;
        if self.image_mode_generation.is_some() {
            return;
        }
        self.report(PacketData::AccelReport(AccelReport {
            timestamp: self.timestamp(),
            accel: self.state.accel.into(),
            gyro: self.state.gyro.into(),
        }));
    }

    fn emit_markers(&mut self) {
        self.last_markers = self.state.time_ms;
        if self.images.is_some() || self.image_mode_generation.is_some() {
            return;
        }
        self.report(PacketData::PocMarkersReport(scene::markers(
            self.state.aim,
            &self.orientation,
            &self.configs[ConfigKind::CameraModelNf as usize],
        )));
    }

    fn emit_battery(&mut self) {
        self.last_battery = Some(self.state.time_ms);
        self.report(PacketData::BatteryReport(BatteryReport {
            percent: self.state.battery_percent,
            charging: self.state.charging,
        }));
    }
}
