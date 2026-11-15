mod accessories;
mod bind_local_socket;
mod device_list;
mod device_tasks;
mod dongle_list;
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub mod firmware;
mod local_socket_stream;
mod message;
mod message_forwarder;
mod migration;
/// Since: Odyssey Desktop 0.6.6.
pub mod policy;
mod run_server;
mod screen_watch;
mod server;
mod server_error;
/// Since: Odyssey Desktop 0.7.0.
#[cfg(feature = "sim")]
pub mod sim;
mod startup_config;

/// Since: Odyssey Desktop 0.7.0.
#[cfg(feature = "tracking-regression")]
pub use device_tasks::tracking_regression;
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub use local_socket_stream::LocalSocketStream;
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub use message::Message;
/// Server startup entry point and options.
///
/// # Since
///
/// - `ServerOptions`: Odyssey Desktop 0.7.5.
/// - `run_server`: Odyssey Desktop 0.5.0 (earliest tagged release).
pub use run_server::{ServerOptions, run_server};
/// Since: Odyssey Desktop 0.5.0 (earliest tagged release).
pub use server_error::ServerError;
