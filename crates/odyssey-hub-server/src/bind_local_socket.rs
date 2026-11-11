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
        "@odyhub.sock"
            .to_ns_name::<GenericNamespaced>()
            .map_err(|error| report_error(error, app_sender))?
    } else {
        "/tmp/odyhub.sock"
            .to_fs_name::<GenericFilePath>()
            .map_err(|error| report_error(error, app_sender))?
    };

    if ClientStream::connect(name.clone()).await.is_ok() {
        let _ = app_sender.send(Message::ServerInit(Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "Another instance is already running",
        ))));
        return Err(ServerError::SocketAlreadyInUse);
    }

    // No hub answered, so the name is free. On Unix bind() still fails with AddrInUse
    // when a hub that crashed or was killed left its socket file behind, and nothing
    // else unlinks it. try_overwrite reclaims such a stale name (unlink and rebind).
    // max_spin_time bounds that retry loop under real contention.
    let listener_opts = ListenerOptions::new()
        .name(name.clone())
        .try_overwrite(true)
        .max_spin_time(RECLAIM_MAX_SPIN_TIME);
    #[cfg(target_os = "windows")]
    #[expect(
        unsafe_code,
        reason = "interprocess exposes DACL setup only as an unsafe fn"
    )]
    let listener_opts = {
        let mut sd = SecurityDescriptor::new().map_err(|error| report_error(error, app_sender))?;
        // SAFETY: set_dacl's contract applies only to a non-null ACL pointer. A null
        // pointer installs a null DACL, which grants every user access to the pipe.
        unsafe {
            sd.set_dacl(std::ptr::null_mut(), false)
                .map_err(|error| report_error(error, app_sender))?;
        }
        listener_opts.security_descriptor(sd)
    };
    let listener = match listener_opts.create_tokio() {
        Ok(listener) => listener,
        Err(error) => return Err(report_error(error, app_sender)),
    };
    let connections = stream::unfold(listener, |listener| async {
        let connection = listener.accept().await;
        Some((connection.map(LocalSocketStream::new), listener))
    });
    Ok((name, connections))
}

fn report_error(error: std::io::Error, app_sender: &mpsc::UnboundedSender<Message>) -> ServerError {
    let _ = app_sender.send(Message::ServerInit(Err(std::io::Error::new(
        error.kind(),
        error.to_string(),
    ))));
    ServerError::from(error)
}
