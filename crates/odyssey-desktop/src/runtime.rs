//! The tokio runtime behind every async hub, USB and file operation.
//!
//! GPUI runs its own executor on the main thread and does not provide a tokio
//! reactor, but the hub client, server and `nusb` need one. Work that needs
//! tokio goes through [`spawn`]; the returned handle can be awaited from a GPUI
//! task, since a tokio `JoinHandle` does not care which executor polls it.

use std::sync::OnceLock;

use tokio::runtime::{Builder, Runtime};
use tokio::task::JoinHandle;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Builds the runtime. Must run once before the first [`spawn`].
pub(crate) fn start() -> std::io::Result<()> {
    let runtime = Builder::new_multi_thread()
        .enable_all()
        .thread_name("odyssey-tokio")
        .build()?;
    // A second call keeps the first runtime.
    let _ = RUNTIME.set(runtime);
    Ok(())
}

fn runtime() -> &'static Runtime {
    RUNTIME
        .get()
        .expect("runtime::start runs before anything spawns")
}

/// Runs `future` on the tokio runtime.
pub(crate) fn spawn<T: Send + 'static>(
    future: impl Future<Output = T> + Send + 'static,
) -> JoinHandle<T> {
    runtime().spawn(future)
}
