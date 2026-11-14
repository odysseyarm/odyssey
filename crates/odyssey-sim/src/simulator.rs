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
/// Since: Odyssey Desktop 0.7.0.
pub use world::synthetic_screen_calibration;

use crate::dataset::SceneScreen;

/// Since: Odyssey Desktop 0.7.5.
pub type Uuid = [u8; 6];

const UUID_PREFIX: [u8; 3] = *b"SIM";
/// The unit and the dongle every simulator starts with.
///
/// Since: Odyssey Desktop 0.7.0.
pub const UNIT_UUID: Uuid = unit_uuid(0);
/// Since: Odyssey Desktop 0.7.0.
pub const DONGLE_UUID: Uuid = dongle_uuid(0);
const CAPACITY: usize = 64;

/// Address of the `index`th simulated unit.
///
/// Since: Odyssey Desktop 0.7.5.
pub const fn unit_uuid(index: u8) -> Uuid {
    [UUID_PREFIX[0], UUID_PREFIX[1], UUID_PREFIX[2], 0, index, 1]
}

/// Address of the `index`th simulated dongle.
///
/// Since: Odyssey Desktop 0.7.5.
pub const fn dongle_uuid(index: u8) -> Uuid {
    [UUID_PREFIX[0], UUID_PREFIX[1], UUID_PREFIX[2], 0, index, 2]
}

/// Every simulated address starts with `SIM`, which tells it from hardware.
///
/// Since: Odyssey Desktop 0.7.5.
pub fn is_simulated(uuid: &Uuid) -> bool {
    uuid[..3] == UUID_PREFIX
}

/// Addresses as twelve hex digits in JSON, the form the hub's tools print.
///
/// Since: Odyssey Desktop 0.7.5.
pub mod uuid_text {
    use serde::{Deserialize as _, Deserializer, Serializer};

    use super::Uuid;

    /// Twelve hex digits, as [`format`] writes them.
    ///
    /// Since: Odyssey Desktop 0.7.5.
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

    /// Since: Odyssey Desktop 0.7.5.
    pub fn format(uuid: &Uuid) -> String {
        let [a, b, c, d, e, f] = uuid;
        format!("{a:02x}{b:02x}{c:02x}{d:02x}{e:02x}{f:02x}")
    }

    /// Serialize a UUID as twelve lowercase hexadecimal digits.
    ///
    /// # Errors
    ///
    /// Returns an error if the serializer cannot encode the string.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn serialize<S: Serializer>(uuid: &Uuid, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format(uuid))
    }

    /// Deserialize a UUID from twelve hexadecimal digits.
    ///
    /// # Errors
    ///
    /// Returns an error if input is not twelve hexadecimal digits or deserialization fails.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Uuid, D::Error> {
        expect(&String::deserialize(deserializer)?)
    }

    /// Since: Odyssey Desktop 0.7.5.
    pub mod option {
        use serde::{Deserialize as _, Deserializer, Serializer};

        use super::Uuid;

        /// Serialize an optional UUID as twelve lowercase hexadecimal digits.
        ///
        /// # Errors
        ///
        /// Returns an error if the serializer cannot encode the string.
        ///
        /// Since: Odyssey Desktop 0.7.5.
        pub fn serialize<S: Serializer>(
            uuid: &Option<Uuid>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            match uuid {
                Some(uuid) => serializer.serialize_some(&super::format(uuid)),
                None => serializer.serialize_none(),
            }
        }

        /// Deserialize an optional UUID from twelve hexadecimal digits.
        ///
        /// # Errors
        ///
        /// Returns an error if input is not twelve hexadecimal digits or deserialization fails.
        ///
        /// Since: Odyssey Desktop 0.7.5.
        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<Uuid>, D::Error> {
            Option::<String>::deserialize(deserializer)?
                .map(|text| super::expect(&text))
                .transpose()
        }
    }
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockMode {
    /// Since: Odyssey Desktop 0.7.0.
    Realtime,
    /// Since: Odyssey Desktop 0.7.0.
    Manual,
}

/// Selects the image dimensions, product identity and recovery lifecycle.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SensorModel {
    /// Since: Odyssey Desktop 0.7.0.
    #[default]
    Pag7665qn,
    /// Since: Odyssey Desktop 0.7.0.
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

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transport {
    /// Since: Odyssey Desktop 0.7.0.
    Usb,
    /// Since: Odyssey Desktop 0.7.0.
    #[default]
    Ble,
}

/// Which unit and dongle a command acts on. An absent address means the one
/// with the lowest address, which is the pair every simulator starts with.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// Since: Odyssey Desktop 0.7.5.
    #[serde(
        default,
        with = "uuid_text::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub unit: Option<Uuid>,
    /// Since: Odyssey Desktop 0.7.5.
    #[serde(
        default,
        with = "uuid_text::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub dongle: Option<Uuid>,
}

impl Target {
    /// Since: Odyssey Desktop 0.7.5.
    pub const fn unit(uuid: Uuid) -> Self {
        Self {
            unit: Some(uuid),
            dongle: None,
        }
    }

    /// Since: Odyssey Desktop 0.7.5.
    pub const fn dongle(uuid: Uuid) -> Self {
        Self {
            unit: None,
            dongle: Some(uuid),
        }
    }
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// A new unit, unplugged and unbonded. Its address must start with `SIM`.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    AddUnit {
        /// Since: Odyssey Desktop 0.7.5.
        #[serde(with = "uuid_text")]
        uuid: Uuid,
    },
    /// Since: Odyssey Desktop 0.7.5.
    RemoveUnit {
        /// Since: Odyssey Desktop 0.7.5.
        #[serde(with = "uuid_text")]
        uuid: Uuid,
    },
    /// Since: Odyssey Desktop 0.7.5.
    AddDongle {
        /// Since: Odyssey Desktop 0.7.5.
        #[serde(with = "uuid_text")]
        uuid: Uuid,
    },
    /// Since: Odyssey Desktop 0.7.5.
    RemoveDongle {
        /// Since: Odyssey Desktop 0.7.5.
        #[serde(with = "uuid_text")]
        uuid: Uuid,
    },
    /// Since: Odyssey Desktop 0.7.0.
    UnitUsb {
        /// Since: Odyssey Desktop 0.7.0.
        plugged: bool,
    },
    /// Since: Odyssey Desktop 0.7.0.
    DongleUsb {
        /// Since: Odyssey Desktop 0.7.0.
        plugged: bool,
    },
    /// Whether the unit can reach its dongle, or one it is pairing with.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    BleRange {
        /// Since: Odyssey Desktop 0.7.0.
        in_range: bool,
    },
    /// Puts the unit and the dongle in pairing mode together.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    Pair {
        /// Since: Odyssey Desktop 0.7.0.
        timeout_ms: u32,
    },
    /// Since: Odyssey Desktop 0.7.0.
    CancelPairing,
    /// Since: Odyssey Desktop 0.7.0.
    ClearBonds,
    /// The displays every unit looks at. Their markers are what units report.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    Screens {
        /// Since: Odyssey Desktop 0.7.5.
        screens: Vec<SceneScreen>,
    },
    /// Where the unit's camera is, in the screens' world frame: +X right, +Y
    /// down, +Z forward. Motion readings follow from successive poses.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    Pose {
        /// Since: Odyssey Desktop 0.7.5.
        position_m: [f64; 3],
        /// Unit quaternion rotating camera vectors into world space, x,y,z,w.
        ///
        /// Since: Odyssey Desktop 0.7.5.
        orientation_xyzw: [f64; 4],
        /// The simulator time ([`Snapshot::time_ms`]) the unit is there. It
        /// moves there steadily from the pose before. A pose drops the poses
        /// given for its time or later. Absent means now.
        ///
        /// Since: Odyssey Desktop 0.7.5.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at_ms: Option<u64>,
    },
    /// Stand two meters squarely in front of this point of the first screen,
    /// normalized to [0, 1].
    ///
    /// Since: Odyssey Desktop 0.7.0.
    Aim {
        /// Since: Odyssey Desktop 0.7.0.
        x: f32,
        /// Since: Odyssey Desktop 0.7.0.
        y: f32,
    },
    /// Acceleration in m/s² and angular velocity in radians/s, in device axes.
    /// The angular velocity turns the camera until the next `Aim` or `Pose`.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    Motion {
        /// Since: Odyssey Desktop 0.7.0.
        accel: [f32; 3],
        /// Since: Odyssey Desktop 0.7.0.
        gyro: [f32; 3],
    },
    /// Since: Odyssey Desktop 0.7.0.
    Shot,
    /// Since: Odyssey Desktop 0.7.0.
    Battery {
        /// Since: Odyssey Desktop 0.7.0.
        percent: u8,
        /// Since: Odyssey Desktop 0.7.0.
        charging: bool,
    },
    /// Since: Odyssey Desktop 0.7.0.
    Advance {
        /// Since: Odyssey Desktop 0.7.0.
        millis: u64,
    },
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScenarioStep {
    /// Virtual delay relative to the preceding step.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub after_ms: u64,
    /// Since: Odyssey Desktop 0.7.5.
    #[serde(flatten)]
    pub target: Target,
    /// Since: Odyssey Desktop 0.7.0.
    pub command: Command,
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UnitState {
    /// Since: Odyssey Desktop 0.7.5.
    #[serde(with = "uuid_text")]
    pub uuid: Uuid,
    /// Since: Odyssey Desktop 0.7.5.
    pub name: String,
    /// Since: Odyssey Desktop 0.7.5.
    pub usb: bool,
    /// Since: Odyssey Desktop 0.7.5.
    pub usb_generation: u64,
    /// Since: Odyssey Desktop 0.7.5.
    pub ble_in_range: bool,
    /// The dongle this unit is connected to over BLE.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    #[serde(with = "uuid_text::option")]
    pub dongle: Option<Uuid>,
    /// Since: Odyssey Desktop 0.7.5.
    pub bonded: bool,
    /// Since: Odyssey Desktop 0.7.5.
    pub pairing: bool,
    /// Since: Odyssey Desktop 0.7.5.
    pub transport: Transport,
    /// The last `Aim`. Absent once the unit is posed directly.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub aim: Option<[f32; 2]>,
    /// Since: Odyssey Desktop 0.7.5.
    pub position_m: [f64; 3],
    /// Since: Odyssey Desktop 0.7.5.
    pub orientation_xyzw: [f64; 4],
    /// Since: Odyssey Desktop 0.7.5.
    pub accel: [f32; 3],
    /// Since: Odyssey Desktop 0.7.5.
    pub gyro: [f32; 3],
    /// Since: Odyssey Desktop 0.7.5.
    pub battery_percent: u8,
    /// Since: Odyssey Desktop 0.7.5.
    pub charging: bool,
    /// Since: Odyssey Desktop 0.7.5.
    pub shots: u64,
}

/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DongleState {
    /// Since: Odyssey Desktop 0.7.5.
    #[serde(with = "uuid_text")]
    pub uuid: Uuid,
    /// Since: Odyssey Desktop 0.7.5.
    pub usb: bool,
    /// Since: Odyssey Desktop 0.7.5.
    pub usb_generation: u64,
    /// Since: Odyssey Desktop 0.7.5.
    pub pairing: bool,
    /// Since: Odyssey Desktop 0.7.5.
    pub bonds: usize,
}

/// Units and dongles are ordered by address.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    /// Since: Odyssey Desktop 0.7.0.
    pub time_ms: u64,
    /// Since: Odyssey Desktop 0.7.5.
    pub units: Vec<UnitState>,
    /// Since: Odyssey Desktop 0.7.5.
    pub dongles: Vec<DongleState>,
    /// Since: Odyssey Desktop 0.7.0.
    pub last_error: Option<String>,
    /// Since: Odyssey Desktop 0.7.0.
    pub running: bool,
}

impl Snapshot {
    /// Since: Odyssey Desktop 0.7.5.
    pub fn unit(&self, uuid: Uuid) -> Option<&UnitState> {
        self.units.iter().find(|unit| unit.uuid == uuid)
    }

    /// Since: Odyssey Desktop 0.7.5.
    pub fn dongle(&self, uuid: Uuid) -> Option<&DongleState> {
        self.dongles.iter().find(|dongle| dongle.uuid == uuid)
    }
}

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug, thiserror::Error)]
pub enum SimError {
    /// Since: Odyssey Desktop 0.7.0.
    #[error("simulator has stopped")]
    Stopped,
    /// Since: Odyssey Desktop 0.7.0.
    #[error("invalid simulator input: {0}")]
    InvalidInput(&'static str),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("unsupported simulated protocol request: {0}")]
    Unsupported(String),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("simulated transport output is full: {0}")]
    Backpressure(&'static str),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("simulator task failed: {0}")]
    Task(String),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("scenario step {index} failed: {source}")]
    Scenario {
        /// Since: Odyssey Desktop 0.7.0.
        index: usize,
        /// Since: Odyssey Desktop 0.7.0.
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

/// Since: Odyssey Desktop 0.7.0.
#[derive(Clone, Debug)]
pub struct SimulatorHandle {
    commands: mpsc::Sender<Request>,
    snapshots: watch::Receiver<Snapshot>,
    errors: broadcast::Sender<SimError>,
}

impl SimulatorHandle {
    /// Since: Odyssey Desktop 0.7.0.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshots.borrow().clone()
    }
    /// Since: Odyssey Desktop 0.7.0.
    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.snapshots.clone()
    }
    /// The wire protocol has no generic error packet. Unsupported requests are
    /// ignored, as in firmware, and reported here instead of receiving a fake ACK.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn errors(&self) -> broadcast::Receiver<SimError> {
        self.errors.subscribe()
    }

    /// Run a command on the first unit and dongle. The snapshot already shows
    /// its effect when this returns.
    ///
    /// # Errors
    ///
    /// Returns an error if simulator has stopped or command is invalid.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub async fn execute(&self, command: Command) -> Result<(), SimError> {
        self.execute_on(Target::default(), command).await
    }

    /// Execute command on selected unit or dongle.
    ///
    /// # Errors
    ///
    /// Returns an error if simulator has stopped or command is invalid.
    ///
    /// Since: Odyssey Desktop 0.7.5.
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

    /// Execute scenario steps in order and return final snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when any scenario step fails or simulator has stopped.
    ///
    /// Since: Odyssey Desktop 0.7.0.
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
///
/// Since: Odyssey Desktop 0.7.0.
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
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn new() -> Self {
        Self::with_clock(ClockMode::Realtime)
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub fn with_clock(clock: ClockMode) -> Self {
        Self::spawn(clock, SensorModel::default())
    }

    /// The legacy model remains image-only after capture until USB reboot.
    ///
    /// Since: Odyssey Desktop 0.7.0.
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

    /// Since: Odyssey Desktop 0.7.0.
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

    /// Opens a host endpoint. Presence is controlled independently with `UnitUsb`.
    /// Reopening replaces the previous endpoint, like claiming a USB interface.
    ///
    /// Since: Odyssey Desktop 0.7.0.
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

    /// Since: Odyssey Desktop 0.7.0.
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

    /// Execute scenario steps in order and return final snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when any scenario step fails or simulator has stopped.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub async fn run_scenario(&self, steps: &[ScenarioStep]) -> Result<Snapshot, SimError> {
        self.handle.run_scenario(steps).await
    }

    /// Stop simulator task.
    ///
    /// # Errors
    ///
    /// Returns an error if simulator task ended unexpectedly.
    ///
    /// Since: Odyssey Desktop 0.7.0.
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
