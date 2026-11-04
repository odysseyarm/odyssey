//! Opt-in simulator control over a user-private local socket, never TCP.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use interprocess::local_socket::tokio::prelude::LocalSocketStream;
use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName as _};
#[cfg(not(unix))]
use interprocess::local_socket::{GenericNamespaced, ToNsName as _};
use interprocess::local_socket::{ListenerOptions, Name};
use odyssey_sim::{Command, ScenarioStep, SimulatorHandle, Snapshot};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio_util::sync::CancellationToken;

const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("simulator local socket: {0}")]
    Io(#[from] io::Error),
    #[error("simulator control JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("simulator: {0}")]
    Model(String),
    #[error("invalid simulator response")]
    InvalidResponse,
    #[error("simulator configuration: {0}")]
    Config(#[from] odyssey_hub_common::config::ConfigError),
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum Request {
    Snapshot,
    Execute(Command),
    Scenario(Vec<ScenarioStep>),
}

#[derive(Serialize, Deserialize)]
struct Response {
    snapshot: Option<Snapshot>,
    error: Option<String>,
}

/// Process-wide runtime opt-in: `--sim`, `--sim-legacy`, or a truthy `ODYSSEY_SIM`.
pub fn enabled() -> bool {
    static ENABLED: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        std::env::args()
            .skip(1)
            .any(|argument| matches!(argument.as_str(), "--sim" | "--sim-legacy"))
            || std::env::var("ODYSSEY_SIM").is_ok_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
    });
    *ENABLED
}

/// Isolate all hub and desktop settings, and seed the synthetic tracker screen.
///
/// Call before any desktop hooks load settings. Repeated calls in a process share
/// the same root and preserve edits made by that process.
pub fn initialize_config() -> Result<(), ControlError> {
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

/// Execute one command in the simulator owned by the running hub.
pub async fn request(command: Command) -> Result<Snapshot, ControlError> {
    send(Request::Execute(command)).await
}

/// Read the running hub's simulator state.
pub async fn snapshot() -> Result<Snapshot, ControlError> {
    send(Request::Snapshot).await
}

/// Execute ordered virtual-time scenario steps in the running hub.
pub async fn run_scenario(steps: &[ScenarioStep]) -> Result<Snapshot, ControlError> {
    send(Request::Scenario(steps.to_vec())).await
}

async fn send(request: Request) -> Result<Snapshot, ControlError> {
    let mut stream = LocalSocketStream::connect(socket_name()?).await?;
    let mut bytes = serde_json::to_vec(&request)?;
    bytes.push(b'\n');
    stream.write_all(&bytes).await?;
    let mut line = String::new();
    BufReader::new(stream)
        .take(MAX_REQUEST_BYTES)
        .read_line(&mut line)
        .await?;
    let response: Response = serde_json::from_str(&line)?;
    match (response.snapshot, response.error) {
        (Some(snapshot), None) => Ok(snapshot),
        (_, Some(error)) => Err(ControlError::Model(error)),
        _ => Err(ControlError::InvalidResponse),
    }
}

pub(crate) async fn start(
    simulator: SimulatorHandle,
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
    Ok(tokio::spawn(async move {
        loop {
            let connection = tokio::select! {
                () = cancel.cancelled() => break,
                connection = listener.accept() => connection,
            };
            match connection {
                Ok(stream) => {
                    let handle = simulator.clone();
                    let cancel = cancel.clone();
                    tokio::spawn(async move {
                        tokio::select! {
                            () = cancel.cancelled() => {}
                            result = serve_connection(stream, handle) => {
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

async fn serve_connection(
    stream: LocalSocketStream,
    handle: SimulatorHandle,
) -> Result<(), ControlError> {
    let mut reader = BufReader::new(stream).take(MAX_REQUEST_BYTES);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let result = match serde_json::from_str::<Request>(&line) {
        Ok(Request::Snapshot) => Ok(handle.snapshot()),
        Ok(Request::Execute(command)) => handle
            .execute(command)
            .await
            .map_err(|error| error.to_string()),
        Ok(Request::Scenario(steps)) => handle
            .run_scenario(&steps)
            .await
            .map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    let response = match result {
        Ok(snapshot) => Response {
            snapshot: Some(snapshot),
            error: None,
        },
        Err(error) => Response {
            snapshot: None,
            error: Some(error),
        },
    };
    let mut bytes = serde_json::to_vec(&response)?;
    bytes.push(b'\n');
    reader.into_inner().into_inner().write_all(&bytes).await?;
    Ok(())
}
