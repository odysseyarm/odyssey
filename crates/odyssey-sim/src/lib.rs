//! Opt-in host model of the Odyssey device protocols. It does not run firmware.
//!
//! Build the hub and desktop with `--features sim`, then launch with `--sim`
//! or `ODYSSEY_SIM=1`. Simulated units and dongles appear next to physical ones
//! and use the hub's own screen calibrations as their world. `--no-discovery`
//! leaves physical devices out, and `--temp-config` runs on a process-isolated
//! configuration seeded with synthetic screen 0. Together they make a run that
//! depends on nothing outside the process.
//! `--sim-legacy` selects the PAJ7025 image/reboot lifecycle in either application.
//! Library consumers select it with [`Simulator::with_sensor_model`].
//!
//! A simulator starts with one unit and one dongle. [`Command::AddUnit`] and
//! [`Command::AddDongle`] add more. A [`Target`] says which a command is for.
//! Units report the markers of the [`Command::Screens`] they are given, seen
//! from their [`Command::Pose`], with motion readings that follow the poses.
//!
//! `odyssey-sim-control --sim` opens manual controls. `--snapshot` prints live
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
//! capture is 98×98 NF/WF. Stop leaves tracking blocked until reboot creates a new
//! USB endpoint. Board pose changes across frames for calibration.
//! This covers host capture and recovery only, with no physical sensor behavior.
//! Firmware flashing stays unsupported and gets no fabricated ACK.

/// Since: Odyssey Desktop 0.7.0.
pub mod dataset;
/// Since: Odyssey Desktop 0.7.0.
pub mod scene_asset;
/// Since: Odyssey Desktop 0.7.0.
pub mod surface;

mod simulator;

/// Simulator commands, handles and state snapshots.
///
/// # Since
///
/// - `ClockMode`: Odyssey Desktop 0.7.0.
/// - `Command`: Odyssey Desktop 0.7.0.
/// - `DONGLE_UUID`: Odyssey Desktop 0.7.0.
/// - `DongleState`: Odyssey Desktop 0.7.5.
/// - `ScenarioStep`: Odyssey Desktop 0.7.0.
/// - `SensorModel`: Odyssey Desktop 0.7.0.
/// - `SimError`: Odyssey Desktop 0.7.0.
/// - `Simulator`: Odyssey Desktop 0.7.0.
/// - `SimulatorHandle`: Odyssey Desktop 0.7.0.
/// - `Snapshot`: Odyssey Desktop 0.7.0.
/// - `Target`: Odyssey Desktop 0.7.5.
/// - `Transport`: Odyssey Desktop 0.7.0.
/// - `UNIT_UUID`: Odyssey Desktop 0.7.0.
/// - `UnitState`: Odyssey Desktop 0.7.5.
/// - `Uuid`: Odyssey Desktop 0.7.5.
/// - `dongle_uuid`: Odyssey Desktop 0.7.5.
/// - `is_simulated`: Odyssey Desktop 0.7.5.
/// - `synthetic_screen_calibration`: Odyssey Desktop 0.7.0.
/// - `unit_uuid`: Odyssey Desktop 0.7.5.
/// - `uuid_text`: Odyssey Desktop 0.7.5.
pub use simulator::{
    ClockMode, Command, DONGLE_UUID, DongleState, ScenarioStep, SensorModel, SimError, Simulator,
    SimulatorHandle, Snapshot, Target, Transport, UNIT_UUID, UnitState, Uuid, dongle_uuid,
    is_simulated, synthetic_screen_calibration, unit_uuid, uuid_text,
};
