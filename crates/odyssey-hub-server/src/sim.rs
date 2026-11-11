//! Opt-in simulator control over a user-private local socket, never TCP.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use interprocess::local_socket::tokio::prelude::LocalSocketStream;
use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName as _};
#[cfg(not(unix))]
use interprocess::local_socket::{GenericNamespaced, ToNsName as _};
use interprocess::local_socket::{ListenerOptions, Name};
use odyssey_sim::dataset::SceneScreen;
use odyssey_sim::{
    Command, ScenarioStep, SensorModel, SimulatorHandle, Snapshot, Target, is_simulated,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio_util::sync::CancellationToken;

use crate::ServerOptions;
use crate::screen_watch::Calibrations;

const MAX_REQUEST_BYTES: u64 = 1024 * 1024;
/// How often the simulator's screens are compared with the hub's calibrations.
const SCREEN_POLL: Duration = Duration::from_millis(250);

/// Since: Odyssey Desktop 0.7.0.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// Since: Odyssey Desktop 0.7.0.
    #[error("simulator local socket: {0}")]
    Io(#[from] io::Error),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("simulator control JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("simulator: {0}")]
    Model(String),
    /// Since: Odyssey Desktop 0.7.0.
    #[error("invalid simulator response")]
    InvalidResponse,
    /// Since: Odyssey Desktop 0.7.0.
    #[error("simulator configuration: {0}")]
    Config(#[from] odyssey_hub_common::config::ConfigError),
}

/// The simulator flags of a command line, the same in the hub and the desktop.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    /// `--sim`, `--sim-legacy`, or a truthy `ODYSSEY_SIM`.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub simulator: Option<SensorModel>,
    /// `--no-discovery`: leave USB devices and the system Bluetooth radio alone.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub no_discovery: bool,
    /// `--temp-config`: keep every setting in a directory of this process.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub temp_config: bool,
}

impl Flags {
    /// Takes the simulator flags out of `arguments`. `environment` is the value
    /// of `ODYSSEY_SIM`.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    pub fn take(arguments: &mut Vec<String>, environment: Option<&str>) -> Self {
        let mut flags = Self::default();
        if environment.is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        }) {
            flags.simulator = Some(SensorModel::default());
        }
        arguments.retain(|argument| {
            match argument.as_str() {
                "--sim" => {
                    flags.simulator.get_or_insert_default();
                }
                "--sim-legacy" => flags.simulator = Some(SensorModel::Paj7025),
                "--no-discovery" => flags.no_discovery = true,
                "--temp-config" => flags.temp_config = true,
                _ => return true,
            }
            false
        });
        flags
    }

    /// Since: Odyssey Desktop 0.7.5.
    pub fn server_options(self) -> ServerOptions {
        ServerOptions {
            simulator: self.simulator,
            no_discovery: self.no_discovery,
        }
    }
}

static TEMP_CONFIG: AtomicBool = AtomicBool::new(false);

/// Whether a device's settings may be written to the configuration. Simulated
/// devices leave the user's own files alone.
pub(crate) fn persists(uuid: [u8; 6]) -> bool {
    !is_simulated(&uuid) || TEMP_CONFIG.load(Ordering::Relaxed)
}

/// Isolate all hub and desktop settings, and seed the synthetic tracker screen.
///
/// Call before any desktop hooks load settings. Repeated calls in a process share
/// the same root and preserve edits made by that process.
///
/// Since: Odyssey Desktop 0.7.5.
///
/// # Errors
///
/// Returns `ControlError::Io` for directory or file failures, `ControlError::Config` when
/// setting or resolving the simulation config root fails, or `ControlError::Json` when
/// writing the synthetic calibration fails.
pub fn use_temp_config() -> Result<(), ControlError> {
    static ROOT: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!("odyssey-sim-{}-{nonce}", std::process::id()))
    });
    let root = &*ROOT;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        match std::fs::DirBuilder::new().mode(0o700).create(root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    odyssey_hub_common::config::set_simulation_config_root(root.clone())?;
    TEMP_CONFIG.store(true, Ordering::Relaxed);
    let screens = odyssey_hub_common::config::screens_dir()?;
    std::fs::create_dir_all(&screens)?;
    let path = screens.join(odyssey_hub_common::config::screen_file_name(0));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => {
            serde_json::to_writer_pretty(file, &odyssey_sim::synthetic_screen_calibration())?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(unix)]
fn socket_name() -> io::Result<Name<'static>> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "HOME is required for simulator IPC",
        )
    })?;
    let directory = home.join(".odyssey-sim");
    match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = std::fs::symlink_metadata(&directory)?;
    if !metadata.is_dir()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != std::fs::metadata(&home)?.uid()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "simulator IPC directory must be a user-owned directory with mode 0700",
        ));
    }
    directory
        .join("control.sock")
        .to_fs_name::<GenericFilePath>()
}

#[cfg(not(unix))]
fn socket_name() -> io::Result<Name<'static>> {
    "odyssey-sim-control".to_ns_name::<GenericNamespaced>()
}

#[derive(Serialize, Deserialize)]
struct Addressed {
    #[serde(flatten)]
    target: Target,
    command: Command,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum Request {
    Snapshot,
    Execute(Addressed),
    Scenario(Vec<ScenarioStep>),
    /// The hub's screen calibrations: what its tracker believes.
    Screens,
    /// Give the simulator the hub's calibrations again after `Command::Screens`
    /// replaced them.
    FollowScreens,
    /// Replace hub calibrations. Refused unless the hub runs on `--temp-config`.
    Calibrate(Vec<SceneScreen>),
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum Response {
    Done,
    Snapshot(Snapshot),
    Screens(Vec<SceneScreen>),
    Error(String),
}

/// A connection to the simulator owned by the running hub.
///
/// Since: Odyssey Desktop 0.7.5.
#[derive(Debug)]
pub struct Control {
    stream: BufReader<LocalSocketStream>,
    line: String,
}

impl Control {
    /// Since: Odyssey Desktop 0.7.5.
    /// # Errors
    ///
    /// Returns `ControlError::Io` when the local socket name or connection fails.
    pub async fn connect() -> Result<Self, ControlError> {
        Ok(Self {
            stream: BufReader::new(LocalSocketStream::connect(socket_name()?).await?),
            line: String::new(),
        })
    }

    async fn call(&mut self, request: &Request) -> Result<Response, ControlError> {
        let mut bytes = serde_json::to_vec(request)?;
        bytes.push(b'\n');
        self.stream.get_mut().write_all(&bytes).await?;
        self.line.clear();
        (&mut self.stream)
            .take(MAX_REQUEST_BYTES)
            .read_line(&mut self.line)
            .await?;
        match serde_json::from_str(&self.line)? {
            Response::Error(error) => Err(ControlError::Model(error)),
            response => Ok(response),
        }
    }

    async fn done(&mut self, request: &Request) -> Result<(), ControlError> {
        match self.call(request).await? {
            Response::Done => Ok(()),
            _ => Err(ControlError::InvalidResponse),
        }
    }

    async fn state(&mut self, request: &Request) -> Result<Snapshot, ControlError> {
        match self.call(request).await? {
            Response::Snapshot(snapshot) => Ok(snapshot),
            _ => Err(ControlError::InvalidResponse),
        }
    }

    /// Since: Odyssey Desktop 0.7.5.
    /// # Errors
    ///
    /// Returns `ControlError::Io` or `ControlError::Json` for local IPC failures,
    /// `ControlError::Model` when the simulator rejects the command, or
    /// `ControlError::InvalidResponse` for a response other than `Done`.
    pub async fn execute(&mut self, target: Target, command: Command) -> Result<(), ControlError> {
        self.done(&Request::Execute(Addressed { target, command }))
            .await
    }

    /// Since: Odyssey Desktop 0.7.5.
    /// # Errors
    ///
    /// Returns `ControlError::Io` or `ControlError::Json` for local IPC failures,
    /// `ControlError::Model` when the simulator rejects the request, or
    /// `ControlError::InvalidResponse` for a response other than `Snapshot`.
    pub async fn snapshot(&mut self) -> Result<Snapshot, ControlError> {
        self.state(&Request::Snapshot).await
    }

    /// Execute ordered virtual-time scenario steps.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    /// # Errors
    ///
    /// Returns `ControlError::Io` or `ControlError::Json` for local IPC failures,
    /// `ControlError::Model` when a scenario step fails, or
    /// `ControlError::InvalidResponse` for a response other than `Snapshot`.
    pub async fn run_scenario(
        &mut self,
        steps: Vec<ScenarioStep>,
    ) -> Result<Snapshot, ControlError> {
        self.state(&Request::Scenario(steps)).await
    }

    /// The hub's screen calibrations: what its tracker believes the displays are.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    /// # Errors
    ///
    /// Returns `ControlError::Io` or `ControlError::Json` for local IPC failures,
    /// `ControlError::Model` when the simulator rejects the request, or
    /// `ControlError::InvalidResponse` for a response other than `Screens`.
    pub async fn screens(&mut self) -> Result<Vec<SceneScreen>, ControlError> {
        match self.call(&Request::Screens).await? {
            Response::Screens(screens) => Ok(screens),
            _ => Err(ControlError::InvalidResponse),
        }
    }

    /// Make the simulated units look at the hub's calibrated screens again.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    /// # Errors
    ///
    /// Returns `ControlError::Io` or `ControlError::Json` for local IPC failures,
    /// `ControlError::Model` when the simulator rejects the request, or
    /// `ControlError::InvalidResponse` for a response other than `Done`.
    pub async fn follow_screens(&mut self) -> Result<(), ControlError> {
        self.done(&Request::FollowScreens).await
    }

    /// Replace calibrations of a hub that runs on `--temp-config`.
    ///
    /// Since: Odyssey Desktop 0.7.5.
    /// # Errors
    ///
    /// Returns `ControlError::Io` or `ControlError::Json` for local IPC failures,
    /// `ControlError::Model` when calibration is refused or invalid, or
    /// `ControlError::InvalidResponse` for a response other than `Done`.
    pub async fn calibrate(&mut self, screens: Vec<SceneScreen>) -> Result<(), ControlError> {
        self.done(&Request::Calibrate(screens)).await
    }
}

struct Server {
    simulator: SimulatorHandle,
    calibrations: Arc<ArcSwap<Calibrations>>,
    /// A client replaced the simulator's screens, so the hub's no longer apply.
    screens_replaced: AtomicBool,
}

impl Server {
    /// The hub's usable calibrations as the simulator's screens.
    fn hub_screens(&self) -> Vec<SceneScreen> {
        self.calibrations
            .load()
            .iter()
            .map(|(id, calibration)| SceneScreen::from_calibration(*id, calibration.clone()))
            .filter(|screen| screen.geometry().is_ok())
            .collect()
    }

    async fn follow_screens(&self) -> Result<(), String> {
        self.simulator
            .execute(Command::Screens {
                screens: self.hub_screens(),
            })
            .await
            .map_err(|error| error.to_string())
    }

    async fn calibrate(&self, screens: Vec<SceneScreen>) -> Result<(), String> {
        if !TEMP_CONFIG.load(Ordering::Relaxed) {
            return Err(
                "the hub runs on the user's configuration; start it with --temp-config to let a simulation calibrate it"
                    .to_owned(),
            );
        }
        for screen in &screens {
            screen.geometry().map_err(|error| error.to_string())?;
        }
        let directory =
            odyssey_hub_common::config::screens_dir().map_err(|error| error.to_string())?;
        for screen in &screens {
            let json = serde_json::to_vec_pretty(&screen.calibration)
                .map_err(|error| error.to_string())?;
            let path = directory.join(odyssey_hub_common::config::screen_file_name(screen.id));
            let temporary = path.with_extension("tmp");
            tokio::fs::write(&temporary, json)
                .await
                .map_err(|error| error.to_string())?;
            tokio::fs::rename(&temporary, &path)
                .await
                .map_err(|error| error.to_string())?;
        }
        // The file watcher would apply these within two polls. This is immediate.
        self.calibrations.rcu(|current| {
            let mut next: Vec<_> = current
                .iter()
                .filter(|(id, _)| screens.iter().all(|screen| screen.id != *id))
                .cloned()
                .collect();
            next.extend(
                screens
                    .iter()
                    .map(|screen| (screen.id, screen.calibration.clone())),
            );
            next.sort_by_key(|(id, _)| *id);
            next.dedup_by_key(|(id, _)| *id);
            next.into_iter().collect::<Calibrations>()
        });
        Ok(())
    }

    async fn handle(&self, request: Request) -> Response {
        let replaces_screens = |command: &Command| matches!(command, Command::Screens { .. });
        let result = match request {
            Request::Snapshot => Ok(Response::Snapshot(self.simulator.snapshot())),
            Request::Execute(Addressed { target, command }) => {
                let replaced = replaces_screens(&command);
                self.simulator
                    .execute_on(target, command)
                    .await
                    .map(|()| {
                        if replaced {
                            self.screens_replaced.store(true, Ordering::Relaxed);
                        }
                        Response::Done
                    })
                    .map_err(|error| error.to_string())
            }
            Request::Scenario(steps) => {
                if steps.iter().any(|step| replaces_screens(&step.command)) {
                    self.screens_replaced.store(true, Ordering::Relaxed);
                }
                self.simulator
                    .run_scenario(&steps)
                    .await
                    .map(Response::Snapshot)
                    .map_err(|error| error.to_string())
            }
            Request::Screens => Ok(Response::Screens(self.hub_screens())),
            Request::FollowScreens => {
                self.screens_replaced.store(false, Ordering::Relaxed);
                self.follow_screens().await.map(|()| Response::Done)
            }
            Request::Calibrate(screens) => self.calibrate(screens).await.map(|()| Response::Done),
        };
        result.unwrap_or_else(Response::Error)
    }

    /// Keep the simulated units looking at the screens the hub is calibrated for.
    async fn follow_hub_screens(&self, cancel: CancellationToken) {
        let mut applied: Option<Arc<Calibrations>> = None;
        loop {
            let current = self.calibrations.load_full();
            if !applied
                .as_ref()
                .is_some_and(|applied| Arc::ptr_eq(applied, &current))
            {
                applied = Some(current);
                if !self.screens_replaced.load(Ordering::Relaxed)
                    && let Err(error) = self.follow_screens().await
                {
                    tracing::warn!("Simulator did not take the hub's screens: {error}");
                }
            }
            tokio::select! {
                () = cancel.cancelled() => break,
                () = tokio::time::sleep(SCREEN_POLL) => {}
            }
        }
    }
}

pub(crate) async fn start(
    simulator: SimulatorHandle,
    calibrations: Arc<ArcSwap<Calibrations>>,
    cancel: CancellationToken,
) -> Result<tokio::task::JoinHandle<()>, ControlError> {
    let name = socket_name()?;
    if LocalSocketStream::connect(name.clone()).await.is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "another simulator control server is running",
        )
        .into());
    }
    let listener = ListenerOptions::new()
        .name(name)
        .try_overwrite(true)
        .max_spin_time(Duration::from_millis(500))
        .create_tokio()?;
    let server = Arc::new(Server {
        simulator,
        calibrations,
        screens_replaced: AtomicBool::new(false),
    });
    tokio::spawn({
        let server = Arc::clone(&server);
        let cancel = cancel.clone();
        async move { server.follow_hub_screens(cancel).await }
    });
    Ok(tokio::spawn(async move {
        loop {
            let connection = tokio::select! {
                () = cancel.cancelled() => break,
                connection = listener.accept() => connection,
            };
            match connection {
                Ok(stream) => {
                    let server = Arc::clone(&server);
                    let cancel = cancel.clone();
                    tokio::spawn(async move {
                        tokio::select! {
                            () = cancel.cancelled() => {}
                            result = serve_connection(stream, &server) => {
                                if let Err(error) = result {
                                    tracing::debug!("Simulator control connection ended: {error}");
                                }
                            }
                        }
                    });
                }
                Err(error) => {
                    tracing::warn!("Simulator control accept failed: {error}");
                    break;
                }
            }
        }
    }))
}

async fn serve_connection(stream: LocalSocketStream, server: &Server) -> Result<(), ControlError> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        (&mut reader)
            .take(MAX_REQUEST_BYTES)
            .read_line(&mut line)
            .await?;
        if !line.ends_with('\n') {
            // The client closed, or sent more than a request may hold.
            return Ok(());
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => server.handle(request).await,
            Err(error) => Response::Error(error.to_string()),
        };
        let mut bytes = serde_json::to_vec(&response)?;
        bytes.push(b'\n');
        reader.get_mut().write_all(&bytes).await?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_leave_other_arguments_and_accept_the_environment() {
        let mut arguments: Vec<String> = ["calibrate", "--sim", "--monitor", "2", "--no-discovery"]
            .map(str::to_owned)
            .into();
        assert_eq!(
            Flags::take(&mut arguments, None),
            Flags {
                simulator: Some(SensorModel::Pag7665qn),
                no_discovery: true,
                temp_config: false,
            }
        );
        assert_eq!(arguments, ["calibrate", "--monitor", "2"]);

        let mut legacy = vec!["--sim-legacy".to_owned(), "--sim".to_owned()];
        assert_eq!(
            Flags::take(&mut legacy, Some("off")).simulator,
            Some(SensorModel::Paj7025)
        );
        assert_eq!(
            Flags::take(&mut Vec::new(), Some("YES")).simulator,
            Some(SensorModel::Pag7665qn)
        );
        assert_eq!(Flags::take(&mut Vec::new(), Some("0")), Flags::default());
    }

    #[test]
    fn requests_address_units_in_the_scenario_file_form() {
        let request: Request = serde_json::from_str(
            r#"{"type":"execute","value":{"unit":"53494d000101","command":{"type":"shot"}}}"#,
        )
        .unwrap();
        let Request::Execute(Addressed { target, command }) = request else {
            panic!("an execute request");
        };
        assert_eq!(target, Target::unit(odyssey_sim::unit_uuid(1)));
        assert!(matches!(command, Command::Shot));
        let unaddressed: Request =
            serde_json::from_str(r#"{"type":"execute","value":{"command":{"type":"shot"}}}"#)
                .unwrap();
        assert!(matches!(
            unaddressed,
            Request::Execute(Addressed { target, .. }) if target == Target::default()
        ));
    }
}
