//! Opt-in host model of the Odyssey device protocols, not a firmware emulator.
//!
//! Build the hub and desktop with `--features sim`, then launch with `--sim`
//! or `ODYSSEY_SIM=1`. Simulation skips physical USB/accessory discovery and
//! uses a process-isolated configuration seeded with synthetic screen 0.
//! `--sim-legacy` selects the PAJ7025 image/reboot lifecycle in either application.
//! Library consumers select it with [`Simulator::with_sensor_model`].
//!
//! `odyssey-sim-control --sim` opens manual controls; `--snapshot` prints live
//! state and `--scenario FILE` executes a JSON array of [`ScenarioStep`] values.
//! Delays advance virtual time, not wall time. Separate IPC calls when a scenario
//! needs to wait for an asynchronous hub consumer to observe a connection change.
//! The control endpoint is a user-private local socket, never a network listener.
//!
//! `cargo run -p odyssey-sim --example smoke` exercises real USB/mux clients.
//! Image capture renders a synthetic 8×6-inner-corner checkerboard through the
//! real bounded packet client, honoring 1–15 requested FPS subject to backpressure.
//! Both profiles produce pixels without object samples. Modern capture is 320×240
//! NF: tracking pauses during capture and resumes on Stop without reboot. Legacy
//! capture is 98×98 NF/WF; Stop leaves tracking blocked until reboot creates a new
//! USB endpoint. Board pose changes across frames for calibration.
//! This exercises host capture and recovery, not physical sensor behavior.
//! Firmware flashing remains unsupported rather than receiving a fabricated ACK.

pub mod dataset;
pub mod scene_asset;
pub mod surface;

mod simulator;

pub use simulator::{
    ClockMode, Command, DONGLE_UUID, ScenarioStep, SensorModel, SimError, Simulator,
    SimulatorHandle, Snapshot, Transport, UNIT_UUID, synthetic_screen_calibration,
};
