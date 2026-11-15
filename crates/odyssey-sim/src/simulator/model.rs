use std::collections::BTreeMap;

use odyssey_protocol::control::device::{self, DeviceMsg};
use odyssey_protocol::control::usb_mux::{self, UsbMuxCtrlMsg};
use odyssey_protocol::control::{BondEntry, BondedDevice};
use odyssey_protocol::mux::MuxMsg;
use odyssey_protocol::{DeviceName, Packet, PacketData};
use tokio::sync::{broadcast, mpsc};

use super::dongle::Dongle;
use super::unit::{Io, Unit};
use super::world::World;
use super::{
    Command, DONGLE_UUID, SensorModel, SimError, Snapshot, Target, UNIT_UUID, Uuid, is_simulated,
};
use crate::dataset::ScenePose;

const MAX_UNITS: usize = 32;
const MAX_DONGLES: usize = 8;
const BOND_KEY: [u8; 16] = [0x53; 16];

pub(crate) struct Errors {
    sender: broadcast::Sender<SimError>,
    last: Option<String>,
}

impl Errors {
    pub(crate) fn report(&mut self, error: SimError) {
        self.last = Some(error.to_string());
        let _ = self.sender.send(error);
    }

    pub(crate) fn unsupported(&mut self, request: impl std::fmt::Debug) {
        self.report(SimError::Unsupported(format!("{request:?}")));
    }
}

pub(crate) fn send<T>(sender: Option<&mpsc::Sender<T>>, message: T) -> Result<(), SimError> {
    match sender {
        Some(sender) => sender.try_send(message).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => SimError::Backpressure("host stopped reading"),
            mpsc::error::TrySendError::Closed(_) => SimError::Stopped,
        }),
        None => Ok(()),
    }
}

pub(crate) struct Model {
    pub(crate) time_ms: u64,
    pub(crate) units: BTreeMap<Uuid, Unit>,
    pub(crate) dongles: BTreeMap<Uuid, Dongle>,
    world: World,
    sensor: SensorModel,
    pub(crate) errors: Errors,
}

impl Model {
    pub(crate) fn new(errors: broadcast::Sender<SimError>, sensor_model: SensorModel) -> Self {
        let world = World::synthetic();
        Self {
            time_ms: 0,
            units: BTreeMap::from([(UNIT_UUID, Unit::new(UNIT_UUID, sensor_model, &world))]),
            dongles: BTreeMap::from([(DONGLE_UUID, Dongle::new(DONGLE_UUID))]),
            world,
            sensor: sensor_model,
            errors: Errors {
                sender: errors,
                last: None,
            },
        }
    }

    pub(crate) fn publish(&self, snapshot: &mut Snapshot) {
        snapshot.time_ms = self.time_ms;
        snapshot
            .units
            .resize_with(self.units.len(), Default::default);
        for (state, unit) in snapshot.units.iter_mut().zip(self.units.values()) {
            unit.write(state);
        }
        snapshot
            .dongles
            .resize_with(self.dongles.len(), Default::default);
        for (state, dongle) in snapshot.dongles.iter_mut().zip(self.dongles.values()) {
            dongle.write(state);
        }
        snapshot.last_error.clone_from(&self.errors.last);
    }

    fn with_unit<R>(
        &mut self,
        uuid: Uuid,
        action: impl FnOnce(&mut Unit, &mut Io<'_>) -> R,
    ) -> Option<R> {
        let unit = self.units.get_mut(&uuid)?;
        let mut io = Io {
            now_ms: self.time_ms,
            ble: unit
                .link
                .and_then(|dongle| self.dongles.get(&dongle))
                .and_then(|dongle| dongle.mux.as_ref()),
            world: &self.world,
            errors: &mut self.errors,
        };
        Some(action(unit, &mut io))
    }

    fn unit_id(&self, target: Target) -> Result<Uuid, SimError> {
        target
            .unit
            .or_else(|| self.units.keys().next().copied())
            .filter(|uuid| self.units.contains_key(uuid))
            .ok_or(SimError::InvalidInput("no such unit"))
    }

    fn dongle_id(&self, target: Target) -> Result<Uuid, SimError> {
        target
            .dongle
            .or_else(|| self.dongles.keys().next().copied())
            .filter(|uuid| self.dongles.contains_key(uuid))
            .ok_or(SimError::InvalidInput("no such dongle"))
    }

    fn on_unit<R>(
        &mut self,
        target: Target,
        action: impl FnOnce(&mut Unit, &mut Io<'_>) -> R,
    ) -> Result<R, SimError> {
        let uuid = self.unit_id(target)?;
        self.with_unit(uuid, action)
            .ok_or(SimError::InvalidInput("no such unit"))
    }

    fn check_new(&self, uuid: Uuid) -> Result<(), SimError> {
        if !is_simulated(&uuid) {
            return Err(SimError::InvalidInput(
                "a simulated address starts with 53494d",
            ));
        }
        if self.units.contains_key(&uuid) || self.dongles.contains_key(&uuid) {
            return Err(SimError::InvalidInput("the address is already in use"));
        }
        Ok(())
    }

    /// A packet the host sent over the unit's USB endpoint.
    pub(crate) fn unit_packet(&mut self, uuid: Uuid, packet: Packet) {
        self.with_unit(uuid, |unit, io| unit.packet(packet, false, io));
    }

    pub(crate) fn unit_control(&mut self, uuid: Uuid, message: DeviceMsg) {
        if let Some(unit) = self.units.get_mut(&uuid) {
            unit.control_message(message, self.time_ms, &mut self.errors);
        }
        self.reconcile();
    }

    pub(crate) fn mux_message(&mut self, uuid: Uuid, message: MuxMsg) {
        let Some(dongle) = self.dongles.get_mut(&uuid) else {
            return;
        };
        if !dongle.plugged {
            return;
        }
        match message {
            MuxMsg::SendTo(request) if dongle.connected.contains(&request.dev) => {
                if let PacketData::SetDeviceName(name) = &request.pkt.data {
                    dongle.bond_names.insert(request.dev, name.clone());
                }
                self.with_unit(request.dev, |unit, io| unit.packet(request.pkt, true, io));
            }
            other => dongle.mux_message(other, &mut self.errors),
        }
    }

    pub(crate) fn dongle_control(&mut self, uuid: Uuid, message: UsbMuxCtrlMsg) {
        if let Some(dongle) = self.dongles.get_mut(&uuid)
            && dongle.plugged
        {
            dongle.control_message(message, self.time_ms, &mut self.errors);
        }
        self.reconcile();
    }

    pub(crate) fn command(&mut self, target: Target, command: Command) -> Result<(), SimError> {
        let result = self.apply(target, command);
        self.reconcile();
        result
    }

    fn apply(&mut self, target: Target, command: Command) -> Result<(), SimError> {
        match command {
            Command::AddUnit { uuid } => {
                self.check_new(uuid)?;
                if self.units.len() >= MAX_UNITS {
                    return Err(SimError::InvalidInput("too many units"));
                }
                self.units
                    .insert(uuid, Unit::new(uuid, self.sensor, &self.world));
            }
            Command::RemoveUnit { uuid } => {
                self.units
                    .remove(&uuid)
                    .ok_or(SimError::InvalidInput("no such unit"))?;
            }
            Command::AddDongle { uuid } => {
                self.check_new(uuid)?;
                if self.dongles.len() >= MAX_DONGLES {
                    return Err(SimError::InvalidInput("too many dongles"));
                }
                self.dongles.insert(uuid, Dongle::new(uuid));
            }
            Command::RemoveDongle { uuid } => {
                self.dongles
                    .remove(&uuid)
                    .ok_or(SimError::InvalidInput("no such dongle"))?;
            }
            Command::UnitUsb { plugged } => {
                self.on_unit(target, |unit, _| unit.set_plugged(plugged))?;
            }
            Command::DongleUsb { plugged } => {
                let uuid = self.dongle_id(target)?;
                if let Some(dongle) = self.dongles.get_mut(&uuid) {
                    dongle.set_plugged(plugged);
                }
            }
            Command::BleRange { in_range } => {
                self.on_unit(target, |unit, _| unit.in_range = in_range)?;
            }
            Command::Pair { timeout_ms } => {
                if timeout_ms == 0 {
                    return Err(SimError::InvalidInput("pairing timeout must be positive"));
                }
                let (unit, dongle) = (self.unit_id(target)?, self.dongle_id(target)?);
                let deadline = Some(self.time_ms.saturating_add(u64::from(timeout_ms)));
                if let Some(unit) = self.units.get_mut(&unit) {
                    unit.pairing_deadline = deadline;
                }
                if let Some(dongle) = self.dongles.get_mut(&dongle) {
                    dongle.pairing_deadline = deadline;
                }
            }
            Command::CancelPairing => {
                let (unit, dongle) = (self.unit_id(target)?, self.dongle_id(target)?);
                if let Some(unit) = self.units.get_mut(&unit) {
                    unit.cancel_pairing(device::PairingError::Cancelled, &mut self.errors);
                }
                if let Some(dongle) = self.dongles.get_mut(&dongle) {
                    dongle.cancel_pairing(usb_mux::PairingError::Cancelled, &mut self.errors);
                }
            }
            Command::ClearBonds => {
                let (unit, dongle) = (self.unit_id(target)?, self.dongle_id(target)?);
                if let Some(unit) = self.units.get_mut(&unit) {
                    unit.pairing_deadline = None;
                    unit.bond = None;
                }
                if let Some(dongle) = self.dongles.get_mut(&dongle) {
                    dongle.pairing_deadline = None;
                    dongle.bonds.clear();
                    dongle.bond_names.clear();
                }
            }
            Command::Screens { screens } => {
                self.world = World::new(screens)?;
                for unit in self.units.values_mut() {
                    unit.follow_screens(&self.world);
                }
            }
            Command::Pose {
                position_m,
                orientation_xyzw,
                at_ms,
            } => {
                let pose = ScenePose {
                    position_m,
                    orientation_xyzw,
                };
                self.on_unit(target, |unit, io| unit.set_pose(pose, at_ms, io.now_ms))??;
            }
            Command::Aim { x, y } => self.on_unit(target, |unit, io| unit.aim(x, y, io))??,
            Command::Motion { accel, gyro } => {
                self.on_unit(target, |unit, io| unit.set_motion(accel, gyro, io))??;
            }
            Command::Shot => self.on_unit(target, Unit::shot)?,
            Command::Battery { percent, charging } => {
                self.on_unit(target, |unit, io| unit.set_battery(percent, charging, io))??;
            }
            Command::Advance { millis } => {
                let Some(end) = self.time_ms.checked_add(millis) else {
                    return Err(SimError::InvalidInput("clock overflow"));
                };
                // Fixed substeps make scripted reporting and timeout order independent of command batching.
                while self.time_ms < end {
                    self.tick(self.time_ms.saturating_add(10).min(end));
                }
            }
        }
        Ok(())
    }

    /// Complete pairings that became possible, then settle which unit is
    /// connected to which dongle.
    pub(crate) fn reconcile(&mut self) {
        while let (Some(dongle), Some(unit)) = (
            self.dongles
                .values_mut()
                .find(|dongle| dongle.plugged && dongle.pairing_deadline.is_some()),
            self.units
                .values_mut()
                .find(|unit| unit.in_range && unit.pairing_deadline.is_some()),
        ) {
            dongle.pairing_deadline = None;
            if !dongle.has_room_for(unit.uuid) {
                dongle.event(
                    UsbMuxCtrlMsg::BondStoreError(usb_mux::BondStoreError::Full),
                    &mut self.errors,
                );
                continue;
            }
            let bond = |peer| BondEntry {
                bd_addr: peer,
                ltk: BOND_KEY,
                security_level: 2,
                is_bonded: true,
                irk: None,
            };
            unit.pairing_deadline = None;
            unit.bond = Some(bond(dongle.uuid));
            dongle.bonds.insert(unit.uuid, bond(unit.uuid));
            let name = DeviceName::from_str_lossy(&unit.name);
            dongle.bond_names.insert(unit.uuid, name.clone());
            unit.event(
                DeviceMsg::PairingResult(device::PairingOutcome::Ok(BondedDevice {
                    uuid: dongle.uuid,
                    name: DeviceName::from_str_lossy("Odyssey Simulated Dongle"),
                })),
                &mut self.errors,
            );
            dongle.event(
                UsbMuxCtrlMsg::PairingResult(usb_mux::PairingOutcome::Ok(BondedDevice {
                    uuid: unit.uuid,
                    name,
                })),
                &mut self.errors,
            );
        }
        for unit in self.units.values_mut() {
            let dongle = unit
                .bond
                .as_ref()
                .filter(|ours| ours.is_bonded)
                .and_then(|ours| {
                    self.dongles.get(&ours.bd_addr).filter(|dongle| {
                        dongle
                            .bonds
                            .get(&unit.uuid)
                            .is_some_and(|theirs| theirs.is_bonded && theirs.ltk == ours.ltk)
                    })
                });
            unit.bonded = dongle.is_some();
            unit.link = dongle
                .filter(|dongle| unit.in_range && dongle.plugged && self.time_ms >= unit.ble_resume)
                .map(|dongle| dongle.uuid);
        }
        for dongle in self.dongles.values_mut() {
            let uuid = dongle.uuid;
            dongle.set_connected(
                self.units
                    .values()
                    .filter(|unit| unit.link == Some(uuid))
                    .map(|unit| &unit.uuid),
                &mut self.errors,
            );
        }
    }

    pub(crate) fn tick(&mut self, time: u64) {
        let dt_s = time.saturating_sub(self.time_ms) as f64 / 1000.0;
        self.time_ms = time;
        for unit in self.units.values_mut() {
            unit.advance(dt_s, time, &mut self.errors);
        }
        for dongle in self.dongles.values_mut() {
            if dongle
                .pairing_deadline
                .is_some_and(|deadline| deadline <= time)
            {
                dongle.cancel_pairing(usb_mux::PairingError::Timeout, &mut self.errors);
            }
        }
        self.reconcile();
        for unit in self.units.values_mut() {
            let mut io = Io {
                now_ms: time,
                ble: unit
                    .link
                    .and_then(|dongle| self.dongles.get(&dongle))
                    .and_then(|dongle| dongle.mux.as_ref()),
                world: &self.world,
                errors: &mut self.errors,
            };
            unit.emit(&mut io);
        }
    }
}

#[cfg(test)]
mod tests {
    use odyssey_protocol::accessory::{self, Error, Profile, Request};
    use odyssey_protocol::{PacketType, StreamUpdate, StreamUpdateAction};

    use super::super::{dongle_uuid, unit_uuid};
    use super::*;

    fn model() -> Model {
        Model::new(broadcast::channel(1).0, SensorModel::default())
    }

    fn run(model: &mut Model, target: Target, command: Command) {
        model.command(target, command).unwrap();
    }

    fn connected(model: &Model, dongle: Uuid) -> &[Uuid] {
        &model.dongles[&dongle].connected
    }

    #[test]
    fn tracker_only_dongle_rejects_accessory_commands_without_timeout() {
        let mut model = model();
        let (sender, mut responses) = mpsc::channel(1);
        run(
            &mut model,
            Target::default(),
            Command::DongleUsb { plugged: true },
        );
        model.dongles.get_mut(&DONGLE_UUID).unwrap().mux = Some(sender);
        let address = [2, 0, 0, 0, 0, 1];
        let commands = [
            accessory::Command::Capabilities(),
            accessory::Command::Scan { enabled: true },
            accessory::Command::Connect {
                address,
                profile: Profile::MarkerControlBoard,
                enroll: true,
            },
            accessory::Command::Disconnect { address },
            accessory::Command::MarkerDemand {
                address,
                active: true,
            },
            accessory::Command::MarkerMode {
                address,
                mode: odyssey_protocol::marker_control::Mode::Auto,
            },
        ];
        for (id, command) in (0..).zip(commands) {
            model.mux_message(
                DONGLE_UUID,
                MuxMsg::AccessoryRequest(Request { id, command }),
            );
            let MuxMsg::AccessoryCompletion(completion) =
                responses.try_recv().expect("immediate accessory response")
            else {
                panic!("accessory request must complete with typed rejection");
            };
            assert_eq!(completion.id, id, "{command:?}");
            assert_eq!(completion.result, Err(Error::Unsupported), "{command:?}");
        }
    }

    #[test]
    fn each_unit_connects_to_the_dongle_it_last_paired_with() {
        let mut model = model();
        let (second_unit, second_dongle) = (unit_uuid(1), dongle_uuid(1));
        run(
            &mut model,
            Target::default(),
            Command::AddUnit { uuid: second_unit },
        );
        run(
            &mut model,
            Target::default(),
            Command::AddDongle {
                uuid: second_dongle,
            },
        );
        for dongle in [DONGLE_UUID, second_dongle] {
            run(
                &mut model,
                Target::dongle(dongle),
                Command::DongleUsb { plugged: true },
            );
        }
        let pair = Command::Pair { timeout_ms: 1000 };
        run(&mut model, Target::default(), pair.clone());
        run(&mut model, Target::unit(second_unit), pair.clone());
        assert_eq!(connected(&model, DONGLE_UUID), [UNIT_UUID, second_unit]);

        let moved = Target {
            unit: Some(second_unit),
            dongle: Some(second_dongle),
        };
        run(&mut model, moved, pair);
        assert_eq!(connected(&model, DONGLE_UUID), [UNIT_UUID]);
        assert_eq!(connected(&model, second_dongle), [second_unit]);

        run(&mut model, moved, Command::BleRange { in_range: false });
        assert_eq!(connected(&model, second_dongle), &[] as &[Uuid]);
        assert!(model.units[&second_unit].bonded);

        run(
            &mut model,
            Target::default(),
            Command::RemoveDongle { uuid: DONGLE_UUID },
        );
        assert_eq!(model.units[&UNIT_UUID].link, None);
        assert!(!model.units[&UNIT_UUID].bonded);
    }

    #[test]
    fn only_simulated_unused_addresses_can_be_added() {
        let mut model = model();
        let add =
            |model: &mut Model, uuid| model.command(Target::default(), Command::AddUnit { uuid });
        assert!(add(&mut model, [1, 2, 3, 4, 5, 6]).is_err());
        assert!(add(&mut model, UNIT_UUID).is_err());
        assert!(add(&mut model, DONGLE_UUID).is_err());
        add(&mut model, unit_uuid(7)).unwrap();
        assert!(
            model
                .command(Target::unit(unit_uuid(9)), Command::Shot)
                .is_err()
        );
    }

    #[test]
    fn units_follow_timed_poses_and_their_gyro_adds_up_to_the_turn() {
        let mut model = model();
        let (sender, mut packets) = mpsc::channel(256);
        run(
            &mut model,
            Target::default(),
            Command::UnitUsb { plugged: true },
        );
        model.units.get_mut(&UNIT_UUID).unwrap().usb = Some(sender);
        model.unit_packet(
            UNIT_UUID,
            Packet {
                id: 1,
                data: PacketData::StreamUpdate(StreamUpdate {
                    packet_id: PacketType::AccelReport(),
                    action: StreamUpdateAction::Enable,
                }),
            },
        );
        let pose = |yaw: f64, at_ms| Command::Pose {
            position_m: [0.6, 0.3, -2.0],
            orientation_xyzw: [0.0, (yaw / 2.0).sin(), 0.0, (yaw / 2.0).cos()],
            at_ms,
        };
        let state = |model: &Model| {
            let mut snapshot = Snapshot::default();
            model.publish(&mut snapshot);
            snapshot.units[0].clone()
        };
        let yaw = |model: &Model| {
            let unit = state(model);
            2.0 * unit.orientation_xyzw[1].atan2(unit.orientation_xyzw[3])
        };
        let advance = |model: &mut Model, millis| {
            run(model, Target::default(), Command::Advance { millis });
        };
        // The turn about device Z the reports add up to, integrated the way the hub does.
        let mut previous = None;
        let mut reported = || {
            let mut sum = 0.0;
            while let Ok(packet) = packets.try_recv() {
                if let PacketData::AccelReport(report) = packet.data {
                    if let Some(at) = previous {
                        sum += f64::from(report.gyro.z) * f64::from(report.timestamp - at) / 1e6;
                    }
                    previous = Some(report.timestamp);
                }
            }
            sum
        };
        run(&mut model, Target::default(), pose(0.0, None));
        assert_eq!(state(&model).aim, None);
        advance(&mut model, 200);
        reported();

        let start = model.time_ms;
        run(&mut model, Target::default(), pose(0.1, Some(start + 100)));
        advance(&mut model, 50);
        assert!((yaw(&model) - 0.05).abs() < 1e-9, "{}", yaw(&model));
        // One radian per second about the camera's Y axis is device axis Z, negated.
        let turning = state(&model);
        assert!((turning.gyro[2] + 1.0).abs() < 1e-4, "{:?}", turning.gyro);
        assert!(turning.gyro[0].abs() < 1e-6 && turning.gyro[1].abs() < 1e-6);
        assert!((turning.accel[2] - 9.80665).abs() < 1e-4);
        advance(&mut model, 100);
        assert!((yaw(&model) - 0.1).abs() < 1e-9);
        assert_eq!(state(&model).gyro, [0.0; 3]);
        assert!((reported() + 0.1).abs() < 1e-6);

        // Faster than a hand: the unit waits, then jumps, and the gyro reads nothing.
        let soon = Some(model.time_ms + 10);
        run(&mut model, Target::default(), pose(1.5, soon));
        advance(&mut model, 50);
        assert!((yaw(&model) - 1.5).abs() < 1e-9);
        assert!(reported().abs() < 1e-9);

        // An untimed pose is due now, reached over the time since the last one.
        run(&mut model, Target::default(), pose(1.55, None));
        advance(&mut model, 20);
        assert!((yaw(&model) - 1.55).abs() < 1e-9);
        assert!((reported() + 0.05).abs() < 1e-6);

        // After a pause it is a jump.
        advance(&mut model, 200);
        run(&mut model, Target::default(), pose(1.6, None));
        assert!((yaw(&model) - 1.6).abs() < 1e-9);
        advance(&mut model, 20);
        assert!(reported().abs() < 1e-9);
    }
}
