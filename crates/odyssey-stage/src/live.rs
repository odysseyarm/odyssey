use std::collections::HashMap;

use odyssey_sim::surface::ScreenGeometry;
use odyssey_sim::{Command, DONGLE_UUID, ScenarioStep, Snapshot, Target, UNIT_UUID, Uuid};

use crate::StageError;
use crate::compiled::{Compiled, LinkReport};
use crate::pattern::Button;
use crate::radio::LinkQuality;
use crate::stage::{Device, DeviceKind};

const PAIRING_TIMEOUT_MS: u32 = 30_000;
/// Margin the link must regain before a simulated unit reconnects; the gap to
/// zero keeps a marginal link from flapping.
const REJOIN_MARGIN_DB: f64 = 3.0;

/// The staged moment the simulator should show at its own time `at_ms`
/// ([`Snapshot::time_ms`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cue {
    pub t_s: f64,
    pub at_ms: u64,
}

type Addressed = (Target, Command);
type Bound<'a> = Vec<(Uuid, &'a Device)>;

/// Turns the staged run into the commands a hub's simulator takes: which units
/// and dongles exist, where each tracker is, its button presses, and whether
/// its dongle can hear it.
#[derive(Debug, Default)]
pub struct SimFeed {
    prepared: bool,
    last_t_s: Option<f64>,
    in_range: HashMap<Uuid, bool>,
    /// The dongle each unit was paired with or last seen on.
    dongle_of: HashMap<Uuid, Uuid>,
}

impl SimFeed {
    /// Commands for the interval since the previous call, ending at `t_s`,
    /// given the simulator's current state. The first call also makes the
    /// simulator's devices match the stage's and connects them. Trackers are
    /// posed as at `cue`, or at `t_s` at once without one. Time must advance;
    /// a rewind (loop or scrub) forgets press history so presses replay
    /// instead of being skipped.
    pub fn step(
        &mut self,
        compiled: &Compiled,
        snapshot: &Snapshot,
        t_s: f64,
        cue: Option<Cue>,
    ) -> Result<Vec<Addressed>, StageError> {
        let (mut units, mut dongles): (Bound<'_>, Bound<'_>) = (Vec::new(), Vec::new());
        for (uuid, device) in compiled.stage().hub_devices() {
            match device.kind {
                DeviceKind::Tracker(_) => units.push((uuid, device)),
                DeviceKind::Dongle => dongles.push((uuid, device)),
            }
        }
        let mut commands = Vec::new();
        if units.is_empty() && dongles.is_empty() {
            return Ok(commands);
        }
        let links = compiled.links(t_s)?;
        if !std::mem::replace(&mut self.prepared, true) {
            self.prepare(&units, &dongles, &links, snapshot, &mut commands);
        }
        let since = match self.last_t_s {
            Some(last) if last <= t_s => last,
            _ => f64::NEG_INFINITY,
        };
        self.last_t_s = Some(t_s);
        let posed_at = cue.map_or((t_s, None), |cue| (cue.t_s, Some(cue.at_ms)));
        let geometry = compiled.geometry()?;
        for (uuid, device) in &units {
            if let Some(dongle) = snapshot.unit(*uuid).and_then(|unit| unit.dongle) {
                self.dongle_of.insert(*uuid, dongle);
            }
            let target = self.target(*uuid);
            let radio = (!dongles.is_empty()).then(|| {
                let dongle = dongles
                    .iter()
                    .find(|(dongle, _)| target.dongle == Some(*dongle))
                    .map(|(_, device)| device.id.as_str());
                (links.as_slice(), dongle)
            });
            let mut in_range = self.in_range.get(uuid).copied();
            track(
                compiled,
                &geometry,
                device,
                target,
                radio,
                (since, t_s),
                posed_at,
                &mut in_range,
                &mut commands,
            )?;
            if let Some(in_range) = in_range {
                self.in_range.insert(*uuid, in_range);
            }
        }
        Ok(commands)
    }

    /// The simulated unit and the dongle it belongs to.
    pub fn target(&self, unit: Uuid) -> Target {
        Target {
            unit: Some(unit),
            dongle: self.dongle_of.get(&unit).copied(),
        }
    }

    /// Make the simulator's devices the stage's: add the missing, remove the
    /// rest, plug the dongles in and pair each tracker with the dongle that
    /// hears it best. Without a dongle a tracker is plugged in over USB.
    fn prepare(
        &mut self,
        units: &Bound<'_>,
        dongles: &Bound<'_>,
        links: &[LinkReport],
        snapshot: &Snapshot,
        out: &mut Vec<Addressed>,
    ) {
        let wanted =
            |uuid: Uuid, bound: &Bound<'_>| bound.iter().any(|(wanted, _)| *wanted == uuid);
        let all = Target::default();
        for (uuid, _) in units {
            if snapshot.unit(*uuid).is_none() {
                out.push((all, Command::AddUnit { uuid: *uuid }));
            }
        }
        for (uuid, _) in dongles {
            if snapshot.dongle(*uuid).is_none() {
                out.push((all, Command::AddDongle { uuid: *uuid }));
            }
        }
        for unit in &snapshot.units {
            if unit.uuid != UNIT_UUID && !wanted(unit.uuid, units) {
                out.push((all, Command::RemoveUnit { uuid: unit.uuid }));
            }
        }
        for dongle in &snapshot.dongles {
            if dongle.uuid != DONGLE_UUID && !wanted(dongle.uuid, dongles) {
                out.push((all, Command::RemoveDongle { uuid: dongle.uuid }));
            }
        }
        for (uuid, _) in dongles {
            if !snapshot.dongle(*uuid).is_some_and(|dongle| dongle.usb) {
                out.push((Target::dongle(*uuid), Command::DongleUsb { plugged: true }));
            }
        }
        for (uuid, device) in units {
            let state = snapshot.unit(*uuid);
            let wireless = !dongles.is_empty();
            if state.is_some_and(|unit| unit.usb) == wireless {
                out.push((Target::unit(*uuid), Command::UnitUsb { plugged: !wireless }));
            }
            if state
                .and_then(|unit| unit.dongle)
                .is_some_and(|dongle| wanted(dongle, dongles))
            {
                continue;
            }
            let strongest = links
                .iter()
                .filter(|link| link.endpoint == device.id)
                .filter_map(|link| {
                    dongles
                        .iter()
                        .find(|(_, dongle)| dongle.id == link.dongle)
                        .map(|(dongle, _)| (*dongle, link.link.rssi_dbm))
                })
                .max_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((dongle, _)) = strongest {
                self.dongle_of.insert(*uuid, dongle);
                out.push((
                    self.target(*uuid),
                    Command::Pair {
                        timeout_ms: PAIRING_TIMEOUT_MS,
                    },
                ));
            }
        }
    }
}

/// One tracker's commands for the interval `(since, t_s]`, posed as at stage
/// time `posed_at.0` for simulator time `posed_at.1`. `radio` is the stage's
/// links and the dongle the tracker belongs to; without a dongle it is the one
/// that hears the tracker best.
#[expect(
    clippy::too_many_arguments,
    reason = "one tracker's step: where it is, whom it reports to, and what was already sent"
)]
fn track(
    compiled: &Compiled,
    geometry: &ScreenGeometry<'_>,
    device: &Device,
    target: Target,
    radio: Option<(&[LinkReport], Option<&str>)>,
    (since, t_s): (f64, f64),
    (pose_t_s, at_ms): (f64, Option<u64>),
    in_range: &mut Option<bool>,
    out: &mut Vec<Addressed>,
) -> Result<(), StageError> {
    let tracker = device
        .tracker()
        .ok_or_else(|| StageError::NotATracker(device.id.clone()))?;
    let pose = compiled.pose(geometry, device, pose_t_s)?;
    let (position_m, orientation_xyzw) = compiled
        .stage()
        .display
        .simulator_pose(pose.position_m, pose.orientation_xyzw);
    out.push((
        target,
        Command::Pose {
            position_m,
            orientation_xyzw,
            at_ms,
        },
    ));
    for press in &tracker.presses {
        if press.at_s > since && press.at_s <= t_s {
            out.push((
                target,
                match press.button {
                    Button::Trigger => Command::Shot,
                    Button::Pair => Command::Pair {
                        timeout_ms: PAIRING_TIMEOUT_MS,
                    },
                },
            ));
        }
    }
    let link = radio.and_then(|(links, dongle)| {
        links.iter().find(|link| {
            link.endpoint == device.id && dongle.map_or(link.best, |dongle| link.dongle == dongle)
        })
    });
    if let Some(link) = link {
        let reachable = match in_range {
            Some(true) => link.link.quality != LinkQuality::Lost,
            _ => link.link.margin_db >= REJOIN_MARGIN_DB,
        };
        if *in_range != Some(reachable) {
            *in_range = Some(reachable);
            out.push((
                target,
                Command::BleRange {
                    in_range: reachable,
                },
            ));
        }
    }
    Ok(())
}

/// One tracker's staged run as an `odyssey-sim-control --scenario` file for
/// the simulator's first unit: the display, then a pose every `interval_ms`,
/// with presses and range changes where they happen.
pub fn scenario(
    compiled: &Compiled,
    device_id: &str,
    interval_ms: u64,
) -> Result<Vec<ScenarioStep>, StageError> {
    if !(10..=1000).contains(&interval_ms) {
        return Err(StageError::invalid("interval must be 10 to 1000 ms"));
    }
    let device = compiled.stage().device(device_id)?;
    let geometry = compiled.geometry()?;
    let duration_ms = (compiled.stage().duration_s * 1000.0) as u64;
    let target = Target::default();
    let mut steps = Vec::new();
    if !compiled.stage().display.is_hub_screen() {
        steps.push(ScenarioStep {
            after_ms: 0,
            target,
            command: Command::Screens {
                screens: vec![compiled.screen().clone()],
            },
        });
    }
    let mut commands = Vec::new();
    let (mut in_range, mut since) = (None, f64::NEG_INFINITY);
    let mut t_ms = 0;
    while t_ms <= duration_ms {
        let t_s = t_ms as f64 / 1000.0;
        let links = compiled.links(t_s)?;
        track(
            compiled,
            &geometry,
            device,
            target,
            Some((&links, None)),
            (since, t_s),
            (t_s, None),
            &mut in_range,
            &mut commands,
        )?;
        since = t_s;
        for (index, (target, command)) in commands.drain(..).enumerate() {
            steps.push(ScenarioStep {
                after_ms: if index == 0 && t_ms > 0 {
                    interval_ms
                } else {
                    0
                },
                target,
                command,
            });
        }
        t_ms += interval_ms;
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use odyssey_sim::{DongleState, UnitState, unit_uuid};

    use super::*;
    use crate::pattern::Press;
    use crate::stage::{Binding, Stage};

    /// The default stage with its tracker and dongle driving the hub.
    fn bound_stage() -> Stage {
        let mut stage = Stage::default();
        for device in &mut stage.devices {
            device.binding = Binding::HubSim;
        }
        stage
    }

    /// A simulator as it starts: one unit and one dongle, both unplugged.
    fn fresh() -> Snapshot {
        Snapshot {
            units: vec![UnitState {
                uuid: UNIT_UUID,
                ..UnitState::default()
            }],
            dongles: vec![DongleState {
                uuid: DONGLE_UUID,
                ..DongleState::default()
            }],
            ..Snapshot::default()
        }
    }

    fn with_presses(stage: &mut Stage, presses: Vec<Press>) {
        if let DeviceKind::Tracker(tracker) = &mut stage.devices[0].kind {
            tracker.presses = presses;
        }
    }

    #[test]
    fn each_press_is_sent_once_and_replayed_after_a_rewind() {
        let mut stage = bound_stage();
        with_presses(
            &mut stage,
            vec![Press {
                at_s: 1.0,
                button: Button::Trigger,
            }],
        );
        let compiled = stage.compile().unwrap();
        let mut feed = SimFeed::default();
        let mut shots = |t_s| {
            feed.step(&compiled, &fresh(), t_s, None)
                .unwrap()
                .iter()
                .filter(|(_, command)| matches!(command, Command::Shot))
                .count()
        };
        assert_eq!(shots(0.5), 0);
        assert_eq!(shots(1.5), 1);
        assert_eq!(shots(2.0), 0);
        assert_eq!(shots(0.0), 0);
        assert_eq!(shots(1.2), 1);
    }

    #[test]
    fn walking_out_of_range_drops_the_link_and_walking_back_restores_it() {
        let mut stage = bound_stage();
        stage.radio.fading_db = 0.0;
        stage.radio.path_loss_exponent = 3.4;
        if let DeviceKind::Tracker(tracker) = &mut stage.devices[0].kind {
            tracker.movement = crate::pattern::Movement::Walk {
                to: [0.6, 0.3, -60.0],
                period_s: 20.0,
            };
        }
        let compiled = stage.compile().unwrap();
        let mut feed = SimFeed::default();
        let mut events = Vec::new();
        for step in 0..=40 {
            for (target, command) in feed
                .step(&compiled, &fresh(), f64::from(step) * 0.5, None)
                .unwrap()
            {
                if let Command::BleRange { in_range } = command {
                    assert_eq!(target.unit, Some(UNIT_UUID));
                    events.push(in_range);
                }
            }
        }
        assert_eq!(events, [true, false, true]);
    }

    #[test]
    fn the_first_step_makes_the_simulator_match_the_stage() {
        let mut stage = bound_stage();
        let mut second = stage.devices[0].clone();
        second.id = "tracker-2".to_owned();
        stage.devices.push(second);
        let compiled = stage.compile().unwrap();
        let mut snapshot = fresh();
        snapshot.units[0].usb = true;
        snapshot.units.push(UnitState {
            uuid: unit_uuid(5),
            ..UnitState::default()
        });
        let mut feed = SimFeed::default();
        let setup: Vec<String> = feed
            .step(&compiled, &snapshot, 0.0, None)
            .unwrap()
            .iter()
            .filter(|(_, command)| {
                !matches!(command, Command::Pose { .. } | Command::BleRange { .. })
            })
            .map(|(target, command)| {
                let unit = target.unit.map_or(0, |unit| unit[4] + 1);
                let json = serde_json::to_value(command).unwrap();
                format!("{unit}:{}", json["type"].as_str().unwrap())
            })
            .collect();
        assert_eq!(
            setup,
            [
                "0:add_unit",
                "0:remove_unit",
                "0:dongle_usb",
                // The first unit was on USB; with a dongle staged it goes wireless.
                "1:unit_usb",
                "1:pair",
                "2:pair",
            ]
        );
        assert_eq!(feed.target(unit_uuid(1)).dongle, Some(DONGLE_UUID));
        let cue = Cue {
            t_s: 0.2,
            at_ms: 1234,
        };
        let later = feed.step(&compiled, &snapshot, 0.1, Some(cue)).unwrap();
        assert!(later.iter().all(|(_, command)| matches!(
            command,
            Command::Pose {
                at_ms: Some(1234),
                ..
            }
        )));
        assert_eq!(later.len(), 2);
    }

    #[test]
    fn a_tracker_without_a_staged_dongle_is_plugged_in_over_usb() {
        let mut stage = Stage::default();
        stage.devices[0].binding = Binding::HubSim;
        let compiled = stage.compile().unwrap();
        let commands = SimFeed::default()
            .step(&compiled, &fresh(), 0.0, None)
            .unwrap();
        assert!(matches!(
            commands[0],
            (target, Command::UnitUsb { plugged: true }) if target.unit == Some(UNIT_UUID)
        ));
        assert!(matches!(commands[1].1, Command::Pose { .. }));
        assert_eq!(commands.len(), 2);
    }

    #[test]
    fn scenario_spans_the_run_and_keeps_each_press_once() {
        let mut stage = Stage {
            duration_s: 2.0,
            ..Stage::default()
        };
        with_presses(
            &mut stage,
            vec![Press {
                at_s: 0.95,
                button: Button::Trigger,
            }],
        );
        let steps = scenario(&stage.compile().unwrap(), "tracker-1", 100).unwrap();
        assert!(matches!(steps[0].command, Command::Screens { .. }));
        assert_eq!(steps.iter().map(|step| step.after_ms).sum::<u64>(), 2000);
        let shots = steps
            .iter()
            .filter(|step| matches!(step.command, Command::Shot))
            .count();
        assert_eq!(shots, 1);
        assert!(scenario(&stage.compile().unwrap(), "tracker-1", 5).is_err());
    }
}
