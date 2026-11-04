//! Opt-in host model of the Odyssey device protocols, not a firmware emulator.

mod image_stream;
mod model;
mod runtime;

mod scene;

use std::sync::{Arc, Mutex};

use odyssey_protocol::Packet;
use odyssey_protocol::control::device::DeviceMsg;
use odyssey_protocol::control::usb_mux::UsbMuxCtrlMsg;
use odyssey_protocol::mux::MuxMsg;
use odyssey_usb::device::{MuxDevice, VmDevice};
pub use scene::synthetic_screen_calibration;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

pub const UNIT_UUID: [u8; 6] = [0x53, 0x49, 0x4d, 0, 0, 1];
pub const DONGLE_UUID: [u8; 6] = [0x53, 0x49, 0x4d, 0, 0, 2];
const CAPACITY: usize = 64;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transport {
    Usb,
    Ble,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    UnitUsb {
        plugged: bool,
    },
    DongleUsb {
        plugged: bool,
    },
    BleRange {
        in_range: bool,
    },
    Pair {
        timeout_ms: u32,
    },
    CancelPairing,
    ClearBonds,
    /// Position on the virtual screen, normalized to [0, 1].
    Aim {
        x: f32,
        y: f32,
    },
    /// Acceleration in m/s² and angular velocity in radians/s, in device axes.
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
    pub command: Command,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub time_ms: u64,
    pub unit_usb: bool,
    pub dongle_usb: bool,
    pub ble_in_range: bool,
    pub ble_connected: bool,
    pub bonded: bool,
    pub unit_pairing: bool,
    pub dongle_pairing: bool,
    pub unit_usb_generation: u64,
    pub dongle_usb_generation: u64,
    pub transport: Transport,
    pub aim: [f32; 2],
    pub accel: [f32; 3],
    pub gyro: [f32; 3],
    pub battery_percent: u8,
    pub charging: bool,
    pub shots: u64,
    pub name: String,
    pub uuid: [u8; 6],
    pub last_error: Option<String>,
    pub running: bool,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            time_ms: 0,
            unit_usb: false,
            dongle_usb: false,
            ble_in_range: true,
            ble_connected: false,
            bonded: false,
            unit_pairing: false,
            dongle_pairing: false,
            unit_usb_generation: 0,
            dongle_usb_generation: 0,
            transport: Transport::Ble,
            aim: [0.5, 0.5],
            accel: [0.0, 0.0, 9.80665],
            gyro: [0.0; 3],
            battery_percent: 100,
            charging: false,
            shots: 0,
            name: "Odyssey Simulator".to_owned(),
            uuid: UNIT_UUID,
            last_error: None,
            running: true,
        }
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

#[derive(Debug, Default)]
struct Pending {
    usb: Option<Connection<Packet, DeviceMsg>>,
    mux: Option<Connection<MuxMsg, UsbMuxCtrlMsg>>,
}

#[derive(Debug)]
struct Request {
    command: Command,
    response: oneshot::Sender<Result<Snapshot, SimError>>,
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

    pub async fn execute(&self, command: Command) -> Result<Snapshot, SimError> {
        let (response, reply) = oneshot::channel();
        self.commands
            .send(Request { command, response })
            .await
            .map_err(|_closed| SimError::Stopped)?;
        reply.await.map_err(|_closed| SimError::Stopped)?
    }

    pub async fn run_scenario(&self, steps: &[ScenarioStep]) -> Result<Snapshot, SimError> {
        for (index, step) in steps.iter().enumerate() {
            if step.after_ms != 0 {
                self.execute(Command::Advance {
                    millis: step.after_ms,
                })
                .await
                .map_err(|source| SimError::Scenario {
                    index,
                    source: Box::new(source),
                })?;
            }
            self.execute(step.command.clone())
                .await
                .map_err(|source| SimError::Scenario {
                    index,
                    source: Box::new(source),
                })?;
        }
        Ok(self.snapshot())
    }
}

/// Owns the one model task. Dropping the owner aborts it and closes all simulated
/// endpoints even when control handles or USB clients remain alive.
#[derive(Debug)]
pub struct Simulator {
    handle: SimulatorHandle,
    pending: Arc<Mutex<Pending>>,
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
    /// Requires an entered Tokio runtime. The default starts physically unplugged.
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
        let (snapshots, snapshot_rx) = watch::channel(Snapshot::default());
        let (errors, _) = broadcast::channel(CAPACITY);
        let pending = Arc::new(Mutex::new(Pending::default()));
        let notify = Arc::new(Notify::new());
        let handle = SimulatorHandle {
            commands,
            snapshots: snapshot_rx,
            errors: errors.clone(),
        };
        let task = tokio::spawn(runtime::run(
            clock,
            sensor_model,
            receiver,
            snapshots.clone(),
            errors,
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

    /// Opens a host endpoint. Presence is controlled independently with UnitUsb.
    /// Reopening replaces the previous endpoint, like claiming a USB interface.
    pub fn connect_unit_usb(&self) -> VmDevice {
        let (outgoing, packets) = mpsc::channel(CAPACITY);
        let (replies, incoming) = mpsc::channel(CAPACITY);
        let (control_outgoing, controls) = mpsc::channel(CAPACITY);
        let (events, control_incoming) = mpsc::channel(CAPACITY);
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .usb = Some(Connection {
            packets,
            replies,
            controls,
            events,
        });
        self.notify.notify_one();
        VmDevice::from_simulated_channels(outgoing, incoming, control_outgoing, control_incoming)
    }

    pub fn connect_dongle(&self) -> MuxDevice {
        let (outgoing, packets) = mpsc::channel(CAPACITY);
        let (replies, incoming) = mpsc::channel(CAPACITY);
        let (control_outgoing, controls) = mpsc::channel(CAPACITY);
        let (events, control_incoming) = mpsc::channel(CAPACITY);
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .mux = Some(Connection {
            packets,
            replies,
            controls,
            events,
        });
        self.notify.notify_one();
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

impl Drop for Simulator {
    fn drop(&mut self) {
        self.snapshots.send_modify(|snapshot| {
            snapshot.running = false;
            snapshot.ble_connected = false;
        });
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
