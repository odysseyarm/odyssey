use std::time::Duration;

use futures::stream::{self, Stream};
use interprocess::local_socket::tokio::prelude::LocalSocketStream as ClientStream;
use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, NameType as _, ToFsName as _,
    ToNsName as _,
};
#[cfg(target_os = "windows")]
use interprocess::os::windows::{
    local_socket::ListenerOptionsExt,
    security_descriptor::{AsSecurityDescriptorMutExt, SecurityDescriptor},
};
use tokio::sync::mpsc;

use crate::{LocalSocketStream, Message, ServerError};

/// How long a listener may keep retrying to reclaim a stale socket name.
const RECLAIM_MAX_SPIN_TIME: Duration = Duration::from_millis(500);

/// Claims the hub's local socket and yields the connections made to it.
///
/// Fails with [`ServerError::SocketAlreadyInUse`] when another hub answers on the
/// socket. Every failure is also reported to the app as `Message::ServerInit`.
pub(crate) async fn bind_local_socket(
    app_sender: &mpsc::UnboundedSender<Message>,
) -> Result<
    (
        Name<'static>,
        impl Stream<Item = std::io::Result<LocalSocketStream>>,
    ),
    ServerError,
> {
    let name = if GenericNamespaced::is_supported() {
        "@odyhub.sock".to_ns_name::<GenericNamespaced>()?
    } else {
        "/tmp/@odyhub.sock".to_fs_name::<GenericFilePath>()?
    };

    // Check if another instance is already running by trying to connect
    if ClientStream::connect(name.clone()).await.is_ok() {
        let _ = app_sender.send(Message::ServerInit(Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "Another instance is already running",
        ))));
        return Err(ServerError::SocketAlreadyInUse);
    }

    // Nobody answered, so the name is free as far as we're concerned. On Unix, though,
    // bind() still fails with AddrInUse if a socket file is left over at that path from a
    // previous run that didn't exit cleanly (crash, force-quit, kill) — nothing here ever
    // unlinks it otherwise. try_overwrite makes the listener reclaim (delete + rebind) a
    // stale name on AddrInUse instead of failing outright; max_spin_time bounds how long
    // that retry loop can run under genuine contention.
    let listener_opts = ListenerOptions::new()
        .name(name.clone())
        .try_overwrite(true)
        .max_spin_time(RECLAIM_MAX_SPIN_TIME);
    #[cfg(target_os = "windows")]
    let listener_opts = {
        let mut sd = SecurityDescriptor::new()?;
        unsafe {
            sd.set_dacl(std::ptr::null_mut(), false)?;
        }
        listener_opts.security_descriptor(sd)
    };
    let listener = match listener_opts.create_tokio() {
        Ok(listener) => listener,
        Err(e) => {
            let error_kind = e.kind();
            let _ = app_sender.send(Message::ServerInit(Err(e)));
            return Err(ServerError::from(std::io::Error::from(error_kind)));
        }
    };
    let connections = stream::unfold(listener, |listener| async {
        let connection = listener.accept().await;
        Some((connection.map(LocalSocketStream::new), listener))
    });
    Ok((name, connections))
}
