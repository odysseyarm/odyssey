//! Shared server state: the compiled stage, the run clock, and the frame every
//! connected page receives.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use odyssey_stage::radio::Radio;
use odyssey_stage::{Binding, Compiled, DeviceKind, LinkReport, Stage};
use serde::Serialize;
use tokio::sync::{broadcast, watch};

use crate::error::ApiError;
use crate::hub::{HubView, uuid_hex};
use crate::sim::SimLink;

pub(crate) const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const SIM_INTERVAL: Duration = Duration::from_millis(50);
/// How far ahead of its clock the simulator is cued, so a pose arrives before
/// the unit has to be there.
pub(crate) const SIM_LEAD: Duration = Duration::from_millis(100);
const CURVE_POINTS: usize = 60;
const CURVE_MAX_M: f64 = 40.0;
/// A hub reading older than this no longer counts as the device's position.
const TRACKING_STALE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub(crate) struct Message {
    pub(crate) event: &'static str,
    pub(crate) data: Arc<str>,
}

#[derive(Debug)]
pub(crate) struct Clock {
    pub(crate) playing: bool,
    pub(crate) looping: bool,
    pub(crate) speed: f64,
    pub(crate) t_s: f64,
    last: Instant,
}

impl Clock {
    fn new() -> Self {
        Self {
            playing: false,
            looping: true,
            speed: 1.0,
            t_s: 0.0,
            last: Instant::now(),
        }
    }

    fn advance(&mut self, duration_s: f64) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        if !self.playing {
            return;
        }
        self.t_s += elapsed * self.speed;
        if self.t_s >= duration_s {
            if self.looping {
                self.t_s = self.t_s.rem_euclid(duration_s);
            } else {
                self.t_s = duration_s;
                self.playing = false;
            }
        }
    }

    pub(crate) fn set(
        &mut self,
        playing: Option<bool>,
        speed: Option<f64>,
        looping: Option<bool>,
        t_s: Option<f64>,
    ) {
        self.last = Instant::now();
        if let Some(playing) = playing {
            self.playing = playing;
        }
        if let Some(speed) = speed {
            self.speed = speed.clamp(0.05, 8.0);
        }
        if let Some(looping) = looping {
            self.looping = looping;
        }
        if let Some(t_s) = t_s {
            self.t_s = t_s.max(0.0);
        }
    }

    /// The stage time `lead` from now if the run carries on as it is.
    fn ahead(&self, lead: Duration, duration_s: f64) -> f64 {
        if !self.playing {
            return self.t_s;
        }
        let t_s = self.t_s + lead.as_secs_f64() * self.speed;
        if t_s < duration_s {
            t_s
        } else if self.looping {
            t_s.rem_euclid(duration_s)
        } else {
            duration_s
        }
    }
}

#[derive(Serialize)]
struct DeviceFrame {
    id: String,
    position_m: [f64; 3],
    /// Unit vector the tracker points along. Absent for dongles.
    forward: Option<[f64; 3]>,
    aim_uv: Option<[f64; 2]>,
    distance_m: Option<f64>,
    /// What the hub's tracker reports for a hub-driven device.
    hub_aim_uv: Option<[f64; 2]>,
    problem: Option<String>,
}

#[derive(Serialize)]
struct Frame {
    t_s: f64,
    playing: bool,
    looping: bool,
    speed: f64,
    duration_s: f64,
    devices: Vec<DeviceFrame>,
    links: Vec<LinkReport>,
    #[serde(flatten)]
    sim: SimStatus,
}

/// How driving the hub's simulator went last time.
#[derive(Clone, Default, Serialize)]
struct SimStatus {
    sim_error: Option<String>,
    /// Why the hub's tracker may disagree with the staged display.
    sim_note: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct Surface {
    columns: u32,
    rows: u32,
    positions_m: Vec<[f64; 3]>,
}

#[derive(Serialize)]
struct RadioInfo {
    range_m: f64,
    tx_power_dbm: f64,
    sensitivity_dbm: f64,
    curve: Vec<[f64; 2]>,
}

/// Everything the page draws that does not change from frame to frame.
#[derive(Serialize)]
pub(crate) struct Derived {
    width_m: f64,
    height_m: f64,
    surface: Option<Surface>,
    markers: [[f64; 3]; 6],
    max_depth_m: f64,
    radio: RadioInfo,
}

pub(crate) struct App {
    stage: RwLock<Arc<Compiled>>,
    clock: Mutex<Clock>,
    pub(crate) sim: tokio::sync::Mutex<SimLink>,
    sim_status: Mutex<SimStatus>,
    pub(crate) hub: watch::Receiver<HubView>,
    pub(crate) messages: broadcast::Sender<Message>,
    save_path: Option<PathBuf>,
    /// Serve the page from this directory instead of the embedded copy, so
    /// the front end can change without rebuilding.
    pub(crate) web_dir: Option<PathBuf>,
}

impl App {
    pub(crate) fn new(
        stage: &Stage,
        hub: watch::Receiver<HubView>,
        save_path: Option<PathBuf>,
        web_dir: Option<PathBuf>,
    ) -> Result<Arc<Self>, ApiError> {
        Ok(Arc::new(Self {
            stage: RwLock::new(Arc::new(stage.compile()?)),
            clock: Mutex::new(Clock::new()),
            sim: tokio::sync::Mutex::new(SimLink::default()),
            sim_status: Mutex::new(SimStatus::default()),
            hub,
            messages: broadcast::channel(16).0,
            save_path,
            web_dir,
        }))
    }

    pub(crate) fn compiled(&self) -> Arc<Compiled> {
        Arc::clone(
            &self
                .stage
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    pub(crate) fn clock(&self) -> std::sync::MutexGuard<'_, Clock> {
        self.clock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Replace the stage. The previous one stays when the new one is invalid.
    pub(crate) fn set_stage(&self, stage: &Stage) -> Result<Arc<Compiled>, ApiError> {
        let compiled = Arc::new(stage.compile()?);
        if let Some(path) = &self.save_path {
            let json = serde_json::to_vec_pretty(stage)
                .map_err(|error| ApiError::Internal(error.to_string()))?;
            let temporary = path.with_extension("tmp");
            std::fs::write(&temporary, json)
                .and_then(|()| std::fs::rename(&temporary, path))
                .map_err(|error| {
                    ApiError::Internal(format!("cannot save {}: {error}", path.display()))
                })?;
        }
        *self
            .stage
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::clone(&compiled);
        Ok(compiled)
    }

    pub(crate) fn derived(compiled: &Compiled) -> Result<Derived, ApiError> {
        let stage = compiled.stage();
        let surface = compiled.screen().calibration.surface.as_ref();
        let max_depth_m = surface.map_or(0.0, |surface| {
            surface
                .positions_m
                .iter()
                .map(|p| p[2].abs())
                .fold(0.0, f64::max)
        });
        let radio: &Radio = &stage.radio;
        Ok(Derived {
            width_m: stage.display.width_m,
            height_m: stage.display.height_m,
            surface: surface.map(|surface| Surface {
                columns: surface.columns,
                rows: surface.rows,
                positions_m: surface.positions_m.clone(),
            }),
            markers: compiled.markers()?,
            max_depth_m,
            radio: RadioInfo {
                range_m: radio.range_m(),
                tx_power_dbm: radio.tx_power_dbm,
                sensitivity_dbm: radio.sensitivity_dbm,
                curve: radio.curve(CURVE_MAX_M, CURVE_POINTS),
            },
        })
    }

    fn frame(&self, hub: &HubView) -> Result<Frame, ApiError> {
        let compiled = self.compiled();
        let stage = compiled.stage();
        let (t_s, playing, looping, speed) = {
            let clock = self.clock();
            (clock.t_s, clock.playing, clock.looping, clock.speed)
        };
        let geometry = compiled.geometry()?;
        let fresh = |uuid: &str| {
            hub.devices
                .iter()
                .find(|device| device.uuid == uuid)
                .and_then(|device| device.tracking.as_ref())
                .filter(|tracking| tracking.at.is_some_and(|at| at.elapsed() < TRACKING_STALE))
        };
        let measured = |device: &odyssey_stage::Device| match &device.binding {
            Binding::Real { uuid } => fresh(uuid).map(|tracking| tracking.position_m),
            _ => None,
        };
        let mut devices = Vec::with_capacity(stage.devices.len());
        for device in &stage.devices {
            let mut frame = DeviceFrame {
                id: device.id.clone(),
                position_m: device.position_m,
                forward: None,
                aim_uv: None,
                distance_m: None,
                hub_aim_uv: None,
                problem: None,
            };
            if let DeviceKind::Tracker(_) = &device.kind {
                match &device.binding {
                    Binding::Real { uuid } => match fresh(uuid) {
                        Some(tracking) => {
                            frame.position_m = tracking.position_m;
                            frame.aim_uv = Some(tracking.aim_uv);
                            frame.distance_m = Some(tracking.distance_m);
                        }
                        None => frame.problem = Some("no tracking from the hub".to_owned()),
                    },
                    binding => match compiled.pose(&geometry, device, t_s) {
                        Ok(pose) => {
                            frame.position_m = pose.position_m;
                            frame.aim_uv = Some(pose.aim_uv);
                            frame.distance_m = Some(pose.distance_m);
                            if *binding == Binding::HubSim {
                                frame.hub_aim_uv = stage
                                    .hub_devices()
                                    .find(|(_, bound)| bound.id == device.id)
                                    .and_then(|(uuid, _)| {
                                        fresh(&uuid_hex(uuid)).map(|tracking| tracking.aim_uv)
                                    });
                                if frame.hub_aim_uv.is_none() {
                                    frame.problem = Some("no tracking from the hub".to_owned());
                                }
                            }
                        }
                        Err(error) => frame.problem = Some(error.to_string()),
                    },
                }
                if let Some(uv) = frame.aim_uv
                    && let Ok(target) = geometry.point_at_uv(uv)
                {
                    let delta: [f64; 3] = std::array::from_fn(|i| target[i] - frame.position_m[i]);
                    let norm = delta.iter().map(|v| v * v).sum::<f64>().sqrt();
                    if norm > 0.0 {
                        frame.forward = Some(delta.map(|v| v / norm));
                    }
                }
            }
            devices.push(frame);
        }
        // Real positions only affect radio links through bound devices.
        let links = compiled.links_with(t_s, measured)?;
        Ok(Frame {
            t_s,
            playing,
            looping,
            speed,
            duration_s: stage.duration_s,
            devices,
            links,
            sim: self
                .sim_status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        })
    }

    /// Advance the run clock, then publish one frame to every page and the
    /// hub's state every tenth frame. Without a page open only the clock runs.
    pub(crate) fn publish(&self, count: u64) {
        self.clock().advance(self.compiled().stage().duration_s);
        if self.messages.receiver_count() == 0 {
            return;
        }
        let hub = self.hub.borrow().clone();
        match self.frame(&hub).and_then(|frame| {
            serde_json::to_string(&frame).map_err(|e| ApiError::Internal(e.to_string()))
        }) {
            Ok(json) => {
                // `send` fails only when the last page closed since the check above.
                let _ = self.messages.send(Message {
                    event: "frame",
                    data: json.into(),
                });
            }
            Err(error) => tracing::debug!("frame skipped: {error:?}"),
        }
        if count % 10 == 0
            && let Ok(json) = serde_json::to_string(&hub)
        {
            let _ = self.messages.send(Message {
                event: "hub",
                data: json.into(),
            });
        }
    }

    /// Feed the hub's simulator from the stage devices bound to it.
    pub(crate) async fn drive_sim(&self) {
        let compiled = self.compiled();
        let (t_s, cue_t_s) = {
            let clock = self.clock();
            (
                clock.t_s,
                clock.ahead(SIM_LEAD, compiled.stage().duration_s),
            )
        };
        let status = {
            let mut link = self.sim.lock().await;
            let outcome = link.drive(&compiled, t_s, cue_t_s).await;
            SimStatus {
                sim_error: outcome.err(),
                sim_note: link.note.clone(),
            }
        };
        *self
            .sim_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = status;
    }
}

pub(crate) const fn sim_interval() -> Duration {
    SIM_INTERVAL
}
