use std::collections::BTreeMap;

use nalgebra::{Quaternion, UnitQuaternion, Vector3};
use odyssey_protocol::control::BondEntry;
use odyssey_protocol::control::device::{self, DeviceMsg, TransportMode};
use odyssey_protocol::image::{IMAGE_LEASE_TIMEOUT_MS, ImageCommand, ImageStatus, ImageStatusCode};
use odyssey_protocol::mux::{DevicePacket, MuxMsg};
use odyssey_protocol::{
    AccelReport, BatteryReport, ConfigKind, DeviceName, GeneralConfig, ImpactReport, Packet,
    PacketData, PacketType, ProductId, PropKind, Props, ReadRegisterResponse, SetDeviceNameResult,
    StreamUpdateAction, Version,
};
use tokio::sync::mpsc;

use super::image_stream::ImageSession;
use super::model::{Errors, send};
use super::world::World;
use super::{SensorModel, SimError, Transport, UnitState, Uuid};
use crate::dataset::ScenePose;

type Registers = BTreeMap<(u8, u8), u8>;

const GRAVITY_M_S2: f64 = 9.80665;
/// How long a unit keeps moving on its last pose change. A pose stream slower
/// than this reads as a series of stops.
const COAST_MS: u64 = 100;
/// Pose changes faster than a hand moves are jumps, not motion.
const MAX_SPIN_RAD_S: f64 = 20.0;
const MAX_SPEED_M_S: f64 = 10.0;

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

fn rotation(pose: &ScenePose) -> UnitQuaternion<f64> {
    let [x, y, z, w] = pose.orientation_xyzw;
    UnitQuaternion::new_normalize(Quaternion::new(w, x, y, z))
}

/// Device axes are the negated x, z, y of camera axes.
fn device_axes(camera: Vector3<f64>) -> [f32; 3] {
    [-camera.x as f32, -camera.z as f32, -camera.y as f32]
}

/// What the accelerometer reads while the unit is held still: the reaction to
/// gravity, which points along world +Y.
fn resting_accel(pose: &ScenePose) -> [f32; 3] {
    device_axes(rotation(pose).inverse_transform_vector(&Vector3::new(0.0, -GRAVITY_M_S2, 0.0)))
}

/// Angular velocity in camera axes and linear velocity in world axes that
/// carry `from` to `to` in `dt_s`. `None` when the change is a jump.
fn motion_between(
    from: &ScenePose,
    to: &ScenePose,
    dt_s: f64,
) -> Option<(Vector3<f64>, Vector3<f64>)> {
    let spin = (rotation(from).inverse() * rotation(to)).scaled_axis() / dt_s;
    let velocity = (Vector3::from(to.position_m) - Vector3::from(from.position_m)) / dt_s;
    (spin.norm() <= MAX_SPIN_RAD_S && velocity.norm() <= MAX_SPEED_M_S).then_some((spin, velocity))
}

/// What a unit needs from the rest of the model while it handles one event.
pub(crate) struct Io<'a> {
    pub(crate) now_ms: u64,
    /// Mux endpoint of the dongle the unit is connected to over BLE.
    pub(crate) ble: Option<&'a mpsc::Sender<MuxMsg>>,
    pub(crate) world: &'a World,
    pub(crate) errors: &'a mut Errors,
}

pub(crate) struct Unit {
    pub(crate) uuid: Uuid,
    pub(crate) name: String,
    pub(crate) plugged: bool,
    pub(crate) usb_generation: u64,
    pub(crate) in_range: bool,
    pub(crate) bonded: bool,
    /// The dongle this unit is connected to over BLE.
    pub(crate) link: Option<Uuid>,
    pub(crate) usb: Option<mpsc::Sender<Packet>>,
    pub(crate) control: Option<mpsc::Sender<DeviceMsg>>,
    pub(crate) bond: Option<BondEntry>,
    pub(crate) pairing_deadline: Option<u64>,
    pub(crate) ble_resume: u64,
    aim: Option<[f32; 2]>,
    pose: ScenePose,
    /// The last `Pose` and when it arrived; motion readings are its difference
    /// to the next one.
    posed: Option<(u64, ScenePose)>,
    velocity_m_s: Vector3<f64>,
    accel: [f32; 3],
    gyro: [f32; 3],
    battery_percent: u8,
    charging: bool,
    shots: u64,
    configs: [GeneralConfig; 7],
    persisted: [GeneralConfig; 7],
    registers: Registers,
    persisted_registers: Registers,
    streams: [Option<u8>; 256],
    last_shot: Option<u64>,
    last_imu: u64,
    last_markers: u64,
    last_battery: Option<u64>,
    reboot_at: Option<u64>,
    images: Option<ImageSession>,
    sensor_model: SensorModel,
    image_mode_generation: Option<u32>,
}

impl Unit {
    pub(crate) fn new(uuid: Uuid, sensor_model: SensorModel, world: &World) -> Self {
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
        let mut unit = Self {
            uuid,
            name: "Odyssey Simulator".to_owned(),
            plugged: false,
            usb_generation: 0,
            in_range: true,
            bonded: false,
            link: None,
            usb: None,
            control: None,
            bond: None,
            pairing_deadline: None,
            ble_resume: 0,
            aim: Some([0.5, 0.5]),
            pose: ScenePose {
                position_m: [0.0; 3],
                orientation_xyzw: [0.0, 0.0, 0.0, 1.0],
            },
            posed: None,
            velocity_m_s: Vector3::zeros(),
            accel: [0.0; 3],
            gyro: [0.0; 3],
            battery_percent: 100,
            charging: false,
            shots: 0,
            persisted: configs.clone(),
            configs,
            registers: reset_registers(sensor_model),
            persisted_registers: reset_registers(sensor_model),
            streams: [None; 256],
            last_shot: None,
            last_imu: 0,
            last_markers: 0,
            last_battery: None,
            reboot_at: None,
            images: None,
            sensor_model,
            image_mode_generation: None,
        };
        unit.accel = resting_accel(&unit.pose);
        unit.follow_screens(world);
        unit
    }

    pub(crate) fn write(&self, state: &mut UnitState) {
        state.uuid = self.uuid;
        state.name.clone_from(&self.name);
        state.usb = self.plugged;
        state.usb_generation = self.usb_generation;
        state.ble_in_range = self.in_range;
        state.dongle = self.link;
        state.bonded = self.bonded;
        state.pairing = self.pairing_deadline.is_some();
        state.transport = if self.plugged {
            Transport::Usb
        } else {
            Transport::Ble
        };
        state.aim = self.aim;
        state.position_m = self.pose.position_m;
        state.orientation_xyzw = self.pose.orientation_xyzw;
        state.accel = self.accel;
        state.gyro = self.gyro;
        state.battery_percent = self.battery_percent;
        state.charging = self.charging;
        state.shots = self.shots;
    }

    pub(crate) fn disconnect_usb(&mut self) {
        self.usb = None;
        self.control = None;
        self.images = None;
    }

    pub(crate) fn set_plugged(&mut self, plugged: bool) {
        if self.plugged != plugged {
            self.usb_generation = self.usb_generation.wrapping_add(1);
        }
        self.plugged = plugged;
        if !plugged {
            self.disconnect_usb();
        }
    }

    pub(crate) fn event(&mut self, event: DeviceMsg, errors: &mut Errors) {
        if let Err(error) = send(self.control.as_ref(), event) {
            errors.report(error);
        }
    }

    pub(crate) fn cancel_pairing(&mut self, error: device::PairingError, errors: &mut Errors) {
        if self.pairing_deadline.take().is_some() {
            self.event(
                DeviceMsg::PairingResult(device::PairingOutcome::Err(error)),
                errors,
            );
        }
    }

    /// Keep an aimed unit in front of its point when the screens change.
    pub(crate) fn follow_screens(&mut self, world: &World) {
        if let Some(aim) = self.aim
            && let Some(pose) = world.aim_pose(aim)
        {
            self.accel = resting_accel(&pose);
            self.pose = pose;
        }
    }

    pub(crate) fn aim(&mut self, x: f32, y: f32, io: &mut Io<'_>) -> Result<(), SimError> {
        if !x.is_finite()
            || !y.is_finite()
            || !(0.0..=1.0).contains(&x)
            || !(0.0..=1.0).contains(&y)
        {
            return Err(SimError::InvalidInput(
                "aim must be finite and normalized to [0, 1]",
            ));
        }
        let pose = io
            .world
            .aim_pose([x, y])
            .ok_or(SimError::InvalidInput("there is no screen to aim at"))?;
        self.aim = Some([x, y]);
        self.accel = resting_accel(&pose);
        self.pose = pose;
        self.posed = None;
        self.velocity_m_s = Vector3::zeros();
        self.emit_markers(io);
        Ok(())
    }

    /// Reports keep their cadence: the next one shows the new pose.
    pub(crate) fn set_pose(&mut self, pose: ScenePose, now_ms: u64) -> Result<(), SimError> {
        let [x, y, z, w] = pose.orientation_xyzw;
        if !pose.position_m.iter().all(|value| value.is_finite())
            || !((x * x + y * y + z * z + w * w).sqrt() - 1.0)
                .abs()
                .le(&1e-6)
        {
            return Err(SimError::InvalidInput(
                "a pose needs a finite position and a unit quaternion",
            ));
        }
        let elapsed_ms = self
            .posed
            .as_ref()
            .map(|(at, _)| now_ms.saturating_sub(*at));
        // Poses within one clock step replace each other; the motion measured
        // into that step stands.
        if elapsed_ms != Some(0) {
            let (spin, velocity) = match (&self.posed, elapsed_ms) {
                (Some((_, last)), Some(ms)) if ms <= COAST_MS => {
                    motion_between(last, &pose, ms as f64 / 1000.0)
                }
                _ => None,
            }
            .unwrap_or_default();
            self.gyro = device_axes(spin);
            self.velocity_m_s = velocity;
        }
        self.accel = resting_accel(&pose);
        self.posed = Some((now_ms, pose.clone()));
        self.pose = pose;
        self.aim = None;
        Ok(())
    }

    pub(crate) fn set_motion(
        &mut self,
        accel: [f32; 3],
        gyro: [f32; 3],
        io: &mut Io<'_>,
    ) -> Result<(), SimError> {
        if accel.iter().chain(gyro.iter()).any(|v| !v.is_finite()) {
            return Err(SimError::InvalidInput("motion must contain finite values"));
        }
        self.accel = accel;
        self.gyro = gyro;
        self.posed = None;
        self.velocity_m_s = Vector3::zeros();
        self.emit_imu(io);
        Ok(())
    }

    pub(crate) fn shot(&mut self, io: &mut Io<'_>) {
        let GeneralConfig::SuppressMs(suppress) = self.configs[1] else {
            unreachable!()
        };
        if self
            .last_shot
            .is_none_or(|time| io.now_ms.saturating_sub(time) >= u64::from(suppress))
        {
            self.last_shot = Some(io.now_ms);
            self.shots = self.shots.wrapping_add(1);
            self.report(
                PacketData::ImpactReport(ImpactReport {
                    timestamp: timestamp(io.now_ms),
                }),
                io,
            );
        }
    }

    pub(crate) fn set_battery(
        &mut self,
        percent: u8,
        charging: bool,
        io: &mut Io<'_>,
    ) -> Result<(), SimError> {
        if percent > 100 {
            return Err(SimError::InvalidInput(
                "battery percent must not exceed 100",
            ));
        }
        self.battery_percent = percent;
        self.charging = charging;
        self.emit_battery(io);
        Ok(())
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
            PropKind::Uuid => Props::Uuid(self.uuid),
            PropKind::ProductId => Props::ProductId(match self.sensor_model {
                SensorModel::Pag7665qn => ProductId::AtsPro,
                SensorModel::Paj7025 => ProductId::AtsLite,
            } as u16),
            PropKind::Name => Props::Name(DeviceName::from_str_lossy(&self.name)),
            PropKind::Version => Props::Version(Version::new([0, 0, 0])),
            // Zero means no net-core firmware release, not a fabricated real release.
            PropKind::NetCoreRelease => Props::NetCoreRelease(0),
        }
    }

    pub(crate) fn packet(&mut self, packet: Packet, ble: bool, io: &mut Io<'_>) {
        if ble && self.link.is_none() || !ble && !self.plugged {
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
                self.name = name.as_str().to_owned();
                Some(PacketData::SetDeviceNameResponse(SetDeviceNameResult::Ok()))
            }
            PacketData::ImageCommand(command) => self
                .image_command(id, command, ble, io.now_ms)
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
                        io.errors.unsupported(update);
                        return;
                    }
                }
                Some(PacketData::Ack())
            }
            data => {
                io.errors.unsupported(data);
                None
            }
        };
        if id == 255 {
            return;
        }
        if let Some(data) = response {
            let packet = Packet { id, data };
            let result = if ble {
                send(
                    io.ble,
                    MuxMsg::DevicePacket(DevicePacket {
                        dev: self.uuid,
                        pkt: packet,
                    }),
                )
            } else {
                send(self.usb.as_ref(), packet)
            };
            if let Err(error) = result {
                io.errors.report(error);
            }
        }
    }

    /// `None` for Renew, which firmware never answers.
    fn image_command(
        &mut self,
        request_id: u8,
        command: ImageCommand,
        ble: bool,
        now_ms: u64,
    ) -> Option<ImageStatus> {
        let (generation, status) = match command {
            ImageCommand::Renew { generation } => {
                if !ble
                    && let Some(session) = &mut self.images
                    && session.generation == generation
                {
                    session.lease_until_ms = now_ms.saturating_add(IMAGE_LEASE_TIMEOUT_MS);
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
                        now_ms.saturating_add(IMAGE_LEASE_TIMEOUT_MS),
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

    pub(crate) fn control_message(&mut self, message: DeviceMsg, now_ms: u64, errors: &mut Errors) {
        if !self.plugged {
            return;
        }
        match message {
            DeviceMsg::ReadProp(kind) => {
                self.event(DeviceMsg::ReadPropResponse(self.prop(kind)), errors);
            }
            DeviceMsg::GetTransportMode() | DeviceMsg::SetTransportMode(_) => {
                self.event(DeviceMsg::TransportModeStatus(TransportMode::Usb), errors);
            }
            DeviceMsg::ReadConfig(kind) => self.event(
                DeviceMsg::ReadConfigResponse(self.configs[kind as usize].clone()),
                errors,
            ),
            DeviceMsg::WriteConfig(config) => {
                self.write_config(config);
                self.event(DeviceMsg::WriteConfigAck(), errors);
            }
            DeviceMsg::FlashSettings() => {
                self.persist();
                self.event(DeviceMsg::FlashSettingsAck(), errors);
            }
            DeviceMsg::SetDeviceName(name) => {
                self.name = name.as_str().to_owned();
                self.event(
                    DeviceMsg::SetDeviceNameResponse(SetDeviceNameResult::Ok()),
                    errors,
                );
            }
            DeviceMsg::StartPairing(parameters) => {
                self.pairing_deadline =
                    Some(now_ms.saturating_add(u64::from(parameters.timeout_ms)));
                self.event(DeviceMsg::StartPairingResponse(), errors);
            }
            DeviceMsg::CancelPairing() => {
                self.cancel_pairing(device::PairingError::Cancelled, errors);
            }
            DeviceMsg::ClearBond() => {
                self.bond = None;
                self.ble_resume = now_ms.saturating_add(10);
                self.event(
                    DeviceMsg::ClearBondResponse(device::ClearBondResult::Ok()),
                    errors,
                );
            }
            DeviceMsg::AddBond(entry) => {
                self.bond = Some(entry);
                self.ble_resume = now_ms.saturating_add(10);
                self.event(
                    DeviceMsg::AddBondResponse(odyssey_protocol::control::AddBondResult::Ok()),
                    errors,
                );
            }
            DeviceMsg::Reboot() => {
                self.event(DeviceMsg::RebootAck(), errors);
                self.configs = self.persisted.clone();
                self.registers = self.persisted_registers.clone();
                self.streams.fill(None);
                self.reboot_at = Some(now_ms.saturating_add(50));
            }
            other => errors.unsupported(other),
        }
    }

    /// Move the unit and its timers forward to `now_ms`.
    pub(crate) fn advance(&mut self, dt_s: f64, now_ms: u64, errors: &mut Errors) {
        if self
            .posed
            .as_ref()
            .is_some_and(|(at, _)| now_ms.saturating_sub(*at) > COAST_MS)
        {
            self.gyro = [0.0; 3];
            self.velocity_m_s = Vector3::zeros();
        }
        if self.gyro != [0.0; 3] || self.velocity_m_s != Vector3::zeros() {
            let [x, y, z] = self.gyro.map(f64::from);
            let turned = rotation(&self.pose)
                * UnitQuaternion::from_scaled_axis(Vector3::new(-x, -z, -y) * dt_s);
            self.pose.orientation_xyzw = [turned.i, turned.j, turned.k, turned.w];
            self.pose.position_m =
                (Vector3::from(self.pose.position_m) + self.velocity_m_s * dt_s).into();
        }
        if self
            .pairing_deadline
            .is_some_and(|deadline| deadline <= now_ms)
        {
            self.cancel_pairing(device::PairingError::Timeout, errors);
        }
        if self.reboot_at.is_some_and(|deadline| deadline <= now_ms) {
            self.reboot_at = None;
            self.image_mode_generation = None;
            // Reset closes the USB session; the manager observes a new generation and reopens it.
            self.disconnect_usb();
            self.usb_generation = self.usb_generation.wrapping_add(1);
        }
    }

    /// Send the periodic reports that are due.
    pub(crate) fn emit(&mut self, io: &mut Io<'_>) {
        let now_ms = io.now_ms;
        let GeneralConfig::AccelConfig(config) = &self.configs[2] else {
            unreachable!()
        };
        let interval = 1000 / u64::from(config.accel_odr.max(1));
        if now_ms.saturating_sub(self.last_imu) >= interval.max(1) {
            self.emit_imu(io);
        }
        if now_ms.saturating_sub(self.last_markers) >= 10 {
            self.emit_markers(io);
        }
        if self
            .last_battery
            .is_none_or(|last| now_ms.saturating_sub(last) >= 1000)
        {
            self.emit_battery(io);
        }
        if self.sensor_model == SensorModel::Pag7665qn
            && let Some(images) = &self.images
            && now_ms >= images.lease_until_ms
        {
            let expired = Packet {
                id: images.request_id,
                data: PacketData::ImageStatus(ImageStatus {
                    generation: images.generation,
                    status: ImageStatusCode::LeaseExpired,
                }),
            };
            self.images = None;
            if let Err(error) = send(self.usb.as_ref(), expired) {
                io.errors.report(error);
            }
        }
        if let Some(images) = &mut self.images
            && self
                .usb
                .as_ref()
                .is_none_or(|sender| !images.tick(now_ms, sender))
        {
            self.images = None;
        }
    }

    fn report(&mut self, data: PacketData, io: &mut Io<'_>) {
        let packet = Packet { id: 0, data };
        let Some(id) = self.streams[u8::from(packet.ty()) as usize] else {
            return;
        };
        let packet = Packet { id, ..packet };
        let result = if self.plugged {
            send(self.usb.as_ref(), packet)
        } else if self.link.is_some() {
            send(
                io.ble,
                MuxMsg::DevicePacket(DevicePacket {
                    dev: self.uuid,
                    pkt: packet,
                }),
            )
        } else {
            return;
        };
        if let Err(error) = result {
            io.errors.report(error);
        }
    }

    fn emit_imu(&mut self, io: &mut Io<'_>) {
        self.last_imu = io.now_ms;
        if self.image_mode_generation.is_some() {
            return;
        }
        self.report(
            PacketData::AccelReport(AccelReport {
                timestamp: timestamp(io.now_ms),
                accel: self.accel.into(),
                gyro: self.gyro.into(),
            }),
            io,
        );
    }

    fn emit_markers(&mut self, io: &mut Io<'_>) {
        self.last_markers = io.now_ms;
        if self.images.is_some() || self.image_mode_generation.is_some() {
            return;
        }
        let report = io.world.markers(
            &self.pose,
            &self.configs[ConfigKind::CameraModelNf as usize],
        );
        self.report(PacketData::PocMarkersReport(report), io);
    }

    fn emit_battery(&mut self, io: &mut Io<'_>) {
        self.last_battery = Some(io.now_ms);
        self.report(
            PacketData::BatteryReport(BatteryReport {
                percent: self.battery_percent,
                charging: self.charging,
            }),
            io,
        );
    }
}

fn timestamp(now_ms: u64) -> u32 {
    now_ms.wrapping_mul(1000) as u32
}
