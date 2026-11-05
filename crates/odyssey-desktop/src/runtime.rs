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

fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .enable_all()
            .thread_name("odyssey-tokio")
            .build()
            .expect("the tokio runtime must start for the app to talk to the hub")
    })
}

/// Runs `future` on the tokio runtime.
pub(crate) fn spawn<T: Send + 'static>(
    future: impl Future<Output = T> + Send + 'static,
) -> JoinHandle<T> {
    runtime().spawn(future)
}
