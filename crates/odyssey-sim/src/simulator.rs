//! Opt-in host model of the Odyssey device protocols, not a firmware emulator.

mod dongle;
mod image_stream;
mod model;
mod runtime;
mod unit;
mod world;

use std::sync::{Arc, Mutex};

use odyssey_protocol::Packet;
use odyssey_protocol::control::device::DeviceMsg;
use odyssey_protocol::control::usb_mux::UsbMuxCtrlMsg;
use odyssey_protocol::mux::MuxMsg;
use odyssey_usb::device::{MuxDevice, VmDevice};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
pub use world::synthetic_screen_calibration;

use crate::dataset::SceneScreen;

pub type Uuid = [u8; 6];

const UUID_PREFIX: [u8; 3] = *b"SIM";
/// The unit and the dongle every simulator starts with.
pub const UNIT_UUID: Uuid = unit_uuid(0);
pub const DONGLE_UUID: Uuid = dongle_uuid(0);
const CAPACITY: usize = 64;

/// Address of the `index`th simulated unit.
pub const fn unit_uuid(index: u8) -> Uuid {
    [UUID_PREFIX[0], UUID_PREFIX[1], UUID_PREFIX[2], 0, index, 1]
}

/// Address of the `index`th simulated dongle.
pub const fn dongle_uuid(index: u8) -> Uuid {
    [UUID_PREFIX[0], UUID_PREFIX[1], UUID_PREFIX[2], 0, index, 2]
}

/// Every simulated address starts with `SIM`, which tells it from hardware.
pub fn is_simulated(uuid: &Uuid) -> bool {
    uuid[..3] == UUID_PREFIX
}

/// Addresses as twelve hex digits in JSON, the form the hub's tools print.
pub mod uuid_text {
    use serde::{Deserialize as _, Deserializer, Serializer};

    use super::Uuid;

    /// Twelve hex digits, as [`format`] writes them.
    pub fn parse(text: &str) -> Option<Uuid> {
        if text.len() != 12 || !text.is_ascii() {
            return None;
        }
        let mut uuid = [0; 6];
        for (index, byte) in uuid.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
        }
        Some(uuid)
    }

    fn expect<E: serde::de::Error>(text: &str) -> Result<Uuid, E> {
        parse(text).ok_or_else(|| E::custom("an address is twelve hex digits"))
    }

    pub fn format(uuid: &Uuid) -> String {
        let [a, b, c, d, e, f] = uuid;
        format!("{a:02x}{b:02x}{c:02x}{d:02x}{e:02x}{f:02x}")
    }

    pub fn serialize<S: Serializer>(uuid: &Uuid, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format(uuid))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Uuid, D::Error> {
        expect(&String::deserialize(deserializer)?)
    }

    pub mod option {
        use serde::{Deserialize as _, Deserializer, Serializer};

        use super::Uuid;

        pub fn serialize<S: Serializer>(
            uuid: &Option<Uuid>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            match uuid {
                Some(uuid) => serializer.serialize_some(&super::format(uuid)),
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<Uuid>, D::Error> {
            Option::<String>::deserialize(deserializer)?
                .map(|text| super::expect(&text))
                .transpose()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockMode {
    Realtime,
    Manual,
}

/// Selects the image dimensions, product identity and recovery lifecycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SensorModel {
    #[default]
    Pag7665qn,
    Paj7025,
}

impl SensorModel {
    const fn image_dimensions(self) -> (u16, u16) {
        match self {
            Self::Pag7665qn => (320, 240),
            Self::Paj7025 => (98, 98),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transport {
    Usb,
    #[default]
    Ble,
}

/// Which unit and dongle a command acts on. An absent address means the one
/// with the lowest address, which is the pair every simulator starts with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    #[serde(
        default,
        with = "uuid_text::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub unit: Option<Uuid>,
    #[serde(
        default,
        with = "uuid_text::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub dongle: Option<Uuid>,
}

impl Target {
    pub const fn unit(uuid: Uuid) -> Self {
        Self {
            unit: Some(uuid),
            dongle: None,
        }
    }

    pub const fn dongle(uuid: Uuid) -> Self {
        Self {
            unit: None,
            dongle: Some(uuid),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// A new unit, unplugged and unbonded. Its address must start with `SIM`.
    AddUnit {
        #[serde(with = "uuid_text")]
        uuid: Uuid,
    },
    RemoveUnit {
        #[serde(with = "uuid_text")]
        uuid: Uuid,
    },
    AddDongle {
        #[serde(with = "uuid_text")]
        uuid: Uuid,
    },
    RemoveDongle {
        #[serde(with = "uuid_text")]
        uuid: Uuid,
    },
    UnitUsb {
        plugged: bool,
    },
    DongleUsb {
        plugged: bool,
    },
    /// Whether the unit can reach its dongle, or one it is pairing with.
    BleRange {
        in_range: bool,
    },
    /// Puts the unit and the dongle in pairing mode together.
    Pair {
        timeout_ms: u32,
    },
    CancelPairing,
    ClearBonds,
    /// The displays every unit looks at. Their markers are what units report.
    Screens {
        screens: Vec<SceneScreen>,
    },
    /// Where the unit's camera is, in the screens' world frame: +X right, +Y
    /// down, +Z forward. Motion readings follow from successive poses.
    Pose {
        position_m: [f64; 3],
        /// Unit quaternion rotating camera vectors into world space, x,y,z,w.
        orientation_xyzw: [f64; 4],
    },
    /// Stand two meters squarely in front of this point of the first screen,
    /// normalized to [0, 1].
    Aim {
        x: f32,
        y: f32,
    },
    /// Acceleration in m/s² and angular velocity in radians/s, in device axes.
    /// The angular velocity turns the camera until the next `Aim` or `Pose`.
    Motion {
        accel: [f32; 3],
        gyro: [f32; 3],
    },
    Shot,
    Battery {
        percent: u8,
        charging: bool,
    },
    Advance {
        millis: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScenarioStep {
    /// Virtual delay relative to the preceding step.
    pub after_ms: u64,
    #[serde(flatten)]
    pub target: Target,
    pub command: Command,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UnitState {
    #[serde(with = "uuid_text")]
    pub uuid: Uuid,
    pub name: String,
    pub usb: bool,
    pub usb_generation: u64,
    pub ble_in_range: bool,
    /// The dongle this unit is connected to over BLE.
    #[serde(with = "uuid_text::option")]
    pub dongle: Option<Uuid>,
    pub bonded: bool,
    pub pairing: bool,
    pub transport: Transport,
    /// The last `Aim`; absent once the unit is posed directly.
    pub aim: Option<[f32; 2]>,
    pub position_m: [f64; 3],
    pub orientation_xyzw: [f64; 4],
    pub accel: [f32; 3],
    pub gyro: [f32; 3],
    pub battery_percent: u8,
    pub charging: bool,
    pub shots: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DongleState {
    #[serde(with = "uuid_text")]
    pub uuid: Uuid,
    pub usb: bool,
    pub usb_generation: u64,
    pub pairing: bool,
    pub bonds: usize,
}

/// Units and dongles are ordered by address.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub time_ms: u64,
    pub units: Vec<UnitState>,
    pub dongles: Vec<DongleState>,
    pub last_error: Option<String>,
    pub running: bool,
}

impl Snapshot {
    pub fn unit(&self, uuid: Uuid) -> Option<&UnitState> {
        self.units.iter().find(|unit| unit.uuid == uuid)
    }

    pub fn dongle(&self, uuid: Uuid) -> Option<&DongleState> {
        self.dongles.iter().find(|dongle| dongle.uuid == uuid)
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum SimError {
    #[error("simulator has stopped")]
    Stopped,
    #[error("invalid simulator input: {0}")]
    InvalidInput(&'static str),
    #[error("unsupported simulated protocol request: {0}")]
    Unsupported(String),
    #[error("simulated transport output is full: {0}")]
    Backpressure(&'static str),
    #[error("simulator task failed: {0}")]
    Task(String),
    #[error("scenario step {index} failed: {source}")]
    Scenario {
        index: usize,
        #[source]
        source: Box<Self>,
    },
}

#[derive(Debug)]
struct Connection<P, C> {
    packets: mpsc::Receiver<P>,
    replies: mpsc::Sender<P>,
    controls: mpsc::Receiver<C>,
    events: mpsc::Sender<C>,
}

/// A host endpoint opened but not yet installed by the model task.
#[derive(Debug)]
enum Pending {
    Unit(Uuid, Connection<Packet, DeviceMsg>),
    Dongle(Uuid, Connection<MuxMsg, UsbMuxCtrlMsg>),
}

#[derive(Debug)]
struct Request {
    target: Target,
    command: Command,
    response: oneshot::Sender<Result<(), SimError>>,
}

#[derive(Clone, Debug)]
pub struct SimulatorHandle {
    commands: mpsc::Sender<Request>,
    snapshots: watch::Receiver<Snapshot>,
    errors: broadcast::Sender<SimError>,
}

impl SimulatorHandle {
    pub fn snapshot(&self) -> Snapshot {
        self.snapshots.borrow().clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.snapshots.clone()
    }
    /// The wire protocol has no generic error packet. Unsupported requests are
    /// ignored, as in firmware, and reported here instead of receiving a fake ACK.
    pub fn errors(&self) -> broadcast::Receiver<SimError> {
        self.errors.subscribe()
    }

    /// Run a command on the first unit and dongle. The snapshot already shows
    /// its effect when this returns.
    pub async fn execute(&self, command: Command) -> Result<(), SimError> {
        self.execute_on(Target::default(), command).await
    }

    pub async fn execute_on(&self, target: Target, command: Command) -> Result<(), SimError> {
        let (response, reply) = oneshot::channel();
        self.commands
            .send(Request {
                target,
                command,
                response,
            })
            .await
            .map_err(|_closed| SimError::Stopped)?;
        reply.await.map_err(|_closed| SimError::Stopped)?
    }

    pub async fn run_scenario(&self, steps: &[ScenarioStep]) -> Result<Snapshot, SimError> {
        for (index, step) in steps.iter().enumerate() {
            let failed = |source| SimError::Scenario {
                index,
                source: Box::new(source),
            };
            if step.after_ms != 0 {
                self.execute(Command::Advance {
                    millis: step.after_ms,
                })
                .await
                .map_err(failed)?;
            }
            self.execute_on(step.target, step.command.clone())
                .await
                .map_err(failed)?;
        }
        Ok(self.snapshot())
    }
}

/// Owns the one model task. Dropping the owner aborts it and closes all simulated
/// endpoints even when control handles or USB clients remain alive.
#[derive(Debug)]
pub struct Simulator {
    handle: SimulatorHandle,
    pending: Arc<Mutex<Vec<Pending>>>,
    notify: Arc<Notify>,
    task: Option<JoinHandle<()>>,
    snapshots: watch::Sender<Snapshot>,
}

impl Default for Simulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Simulator {
    /// Requires an entered Tokio runtime. The default starts with one unit and
    /// one dongle, both physically unplugged, in front of a synthetic screen.
    pub fn new() -> Self {
        Self::with_clock(ClockMode::Realtime)
    }

    pub fn with_clock(clock: ClockMode) -> Self {
        Self::spawn(clock, SensorModel::default())
    }

    /// The legacy model remains image-only after capture until USB reboot.
    pub fn with_sensor_model(sensor_model: SensorModel) -> Self {
        Self::spawn(ClockMode::Realtime, sensor_model)
    }

    fn spawn(clock: ClockMode, sensor_model: SensorModel) -> Self {
        let (commands, receiver) = mpsc::channel(CAPACITY);
        let (errors, _) = broadcast::channel(CAPACITY);
        let model = model::Model::new(errors.clone(), sensor_model);
        let mut initial = Snapshot {
            running: true,
            ..Snapshot::default()
        };
        model.publish(&mut initial);
        let (snapshots, snapshot_rx) = watch::channel(initial);
        let pending = Arc::new(Mutex::new(Vec::new()));
        let notify = Arc::new(Notify::new());
        let handle = SimulatorHandle {
            commands,
            snapshots: snapshot_rx,
            errors,
        };
        let task = tokio::spawn(runtime::run(
            clock,
            model,
            receiver,
            snapshots.clone(),
            Arc::clone(&pending),
            Arc::clone(&notify),
        ));
        Self {
            handle,
            pending,
            notify,
            task: Some(task),
            snapshots,
        }
    }

    pub fn handle(&self) -> SimulatorHandle {
        self.handle.clone()
    }

    fn open(&self, connection: Pending) {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(connection);
        self.notify.notify_one();
    }

    /// Opens a host endpoint. Presence is controlled independently with UnitUsb.
    /// Reopening replaces the previous endpoint, like claiming a USB interface.
    pub fn connect_unit_usb(&self, uuid: Uuid) -> VmDevice {
        let (outgoing, packets) = mpsc::channel(CAPACITY);
        let (replies, incoming) = mpsc::channel(CAPACITY);
        let (control_outgoing, controls) = mpsc::channel(CAPACITY);
        let (events, control_incoming) = mpsc::channel(CAPACITY);
        self.open(Pending::Unit(
            uuid,
            Connection {
                packets,
                replies,
                controls,
                events,
            },
        ));
        VmDevice::from_simulated_channels(outgoing, incoming, control_outgoing, control_incoming)
    }

    pub fn connect_dongle(&self, uuid: Uuid) -> MuxDevice {
        let (outgoing, packets) = mpsc::channel(CAPACITY);
        let (replies, incoming) = mpsc::channel(CAPACITY);
        let (control_outgoing, controls) = mpsc::channel(CAPACITY);
        let (events, control_incoming) = mpsc::channel(CAPACITY);
        self.open(Pending::Dongle(
            uuid,
            Connection {
                packets,
                replies,
                controls,
                events,
            },
        ));
        MuxDevice::from_simulated_channels(outgoing, incoming, control_outgoing, control_incoming)
    }

    pub async fn run_scenario(&self, steps: &[ScenarioStep]) -> Result<Snapshot, SimError> {
        self.handle.run_scenario(steps).await
    }

    pub async fn shutdown(mut self) -> Result<(), SimError> {
        if let Some(task) = self.task.take() {
            task.abort();
            match task.await {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {}
                Err(error) => return Err(SimError::Task(error.to_string())),
            }
        }
        Ok(())
    }
}

/// What observers see once the model task is gone.
fn mark_stopped(snapshot: &mut Snapshot) {
    snapshot.running = false;
    for unit in &mut snapshot.units {
        unit.dongle = None;
    }
}

impl Drop for Simulator {
    fn drop(&mut self) {
        self.snapshots.send_modify(mark_stopped);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
