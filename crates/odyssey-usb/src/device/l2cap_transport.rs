//! A unit reached over the host's own Bluetooth adapter, through `BlueZ` kernel sockets.
//!
//! The unit is the peripheral. It accepts two LE credit-based L2CAP channels, the
//! control PSM first and the data PSM second, and carries one postcard-encoded
//! [`Packet`] per SDU on each (`firmware/atslite-common/src/ble/peripheral.rs`,
//! `custom_task`). It closes the link unless the central is a stored bond and the
//! link is encrypted, so both sockets ask the kernel for medium security before they
//! connect.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::packets::vm::Packet;
use crate::{Error, Result};

/// PSM of the control channel: commands and their responses
/// (`firmware/atslite-common/src/ble/peripheral.rs`, `firmware/dongle-fw/src/ble/central.rs`).
const PSM_CONTROL: u16 = 0x0080;
/// PSM of the data channel: streamed reports.
const PSM_DATA: u16 = 0x0081;

/// Larger than the biggest SDU the unit sends (data channel MTU 2504), so `recv`
/// never truncates a packet.
const RECV_BUFFER_SIZE: usize = 4096;

/// Per channel. The unit gives up on an unanswered accept after 10 s, and the
/// kernel first has to find the unit's advertisement and encrypt the link.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// `BDADDR_ANY`: let the kernel pick the adapter.
const BDADDR_ANY: [u8; 6] = [0; 6];
/// `BDADDR_LE_PUBLIC` from `include/net/bluetooth/bluetooth.h`. The local side only
/// has to say that the socket is an LE one.
const BDADDR_LE_PUBLIC: u8 = 0x01;
/// `BDADDR_LE_RANDOM` from `include/net/bluetooth/bluetooth.h`. The firmware
/// advertises with a static random address, `odyssey_protocol::control::ble_address`.
const BDADDR_LE_RANDOM: u8 = 0x02;
/// `BTPROTO_L2CAP` from `include/net/bluetooth/bluetooth.h`.
const BTPROTO_L2CAP: libc::c_int = 0;
/// `BT_SECURITY` from `include/net/bluetooth/bluetooth.h`, a `SOL_BLUETOOTH` option.
const BT_SECURITY: libc::c_int = 4;
/// `BT_SECURITY_MEDIUM`: encrypted link, unauthenticated pairing allowed.
const BT_SECURITY_MEDIUM: u8 = 2;

/// `struct sockaddr_l2` from `include/net/bluetooth/l2cap.h`. The `libc` crate has no
/// definition of it.
///
/// `l2_psm` and `l2_cid` are stored little-endian. `l2_bdaddr` is a `bdaddr_t`, least significant byte first: the order the unit's
/// firmware stores its address in and the order of an `odyssey` device uuid. `BlueZ`
/// tools print it reversed.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
#[expect(
    clippy::struct_field_names,
    reason = "field names match the kernel's struct sockaddr_l2"
)]
struct SockAddrL2 {
    l2_family: libc::sa_family_t,
    l2_psm: u16,
    l2_bdaddr: [u8; 6],
    l2_cid: u16,
    l2_bdaddr_type: u8,
}

impl SockAddrL2 {
    fn new(psm: u16, bdaddr: [u8; 6], bdaddr_type: u8) -> Self {
        Self {
            l2_family: libc::sa_family_t::try_from(libc::AF_BLUETOOTH)
                .expect("AF_BLUETOOTH (31) fits sa_family_t"),
            l2_psm: psm.to_le(),
            l2_bdaddr: bdaddr,
            l2_cid: 0,
            l2_bdaddr_type: bdaddr_type,
        }
    }
}

/// The two L2CAP channels of a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Channel {
    Control,
    Data,
}

impl Channel {
    fn for_packet(packet: &Packet) -> Self {
        if packet.is_control() {
            Self::Control
        } else {
            Self::Data
        }
    }

    fn psm(self) -> u16 {
        match self {
            Self::Control => PSM_CONTROL,
            Self::Data => PSM_DATA,
        }
    }
}

#[expect(
    unsafe_code,
    reason = "BlueZ L2CAP sockets: neither std nor libc define the sockaddr or the socket options"
)]
mod sys {
    use std::io;
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};

    use super::{BT_SECURITY, BT_SECURITY_MEDIUM, BTPROTO_L2CAP, SockAddrL2};

    fn check(ret: libc::c_int) -> io::Result<libc::c_int> {
        if ret == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret)
        }
    }

    pub(super) fn socket() -> io::Result<OwnedFd> {
        // SAFETY: socket(2) with constant arguments touches no Rust memory.
        let fd = check(unsafe {
            libc::socket(
                libc::AF_BLUETOOTH,
                libc::SOCK_SEQPACKET | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                BTPROTO_L2CAP,
            )
        })?;
        // SAFETY: `fd` was just returned by socket(2) and nothing else owns it.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Must run before `connect`: the kernel evaluates the level while connecting.
    pub(super) fn require_medium_security(fd: &OwnedFd) -> io::Result<()> {
        /// `struct bt_security`.
        #[repr(C)]
        struct BtSecurity {
            level: u8,
            key_size: u8,
        }
        let security = BtSecurity {
            level: BT_SECURITY_MEDIUM,
            key_size: 0,
        };
        // SAFETY: the pointer and length describe `security`, which outlives the call.
        check(unsafe {
            libc::setsockopt(
                fd.as_raw_fd(),
                libc::SOL_BLUETOOTH,
                BT_SECURITY,
                (&raw const security).cast(),
                size_of::<BtSecurity>() as libc::socklen_t,
            )
        })
        .map(drop)
    }

    pub(super) fn bind(fd: &OwnedFd, address: &SockAddrL2) -> io::Result<()> {
        // SAFETY: the pointer and length describe `address`, which outlives the call.
        check(unsafe {
            libc::bind(
                fd.as_raw_fd(),
                std::ptr::from_ref(address).cast(),
                size_of::<SockAddrL2>() as libc::socklen_t,
            )
        })
        .map(drop)
    }

    /// `EINPROGRESS` is the normal result on a non-blocking socket.
    pub(super) fn connect(fd: &OwnedFd, address: &SockAddrL2) -> io::Result<()> {
        // SAFETY: the pointer and length describe `address`, which outlives the call.
        check(unsafe {
            libc::connect(
                fd.as_raw_fd(),
                std::ptr::from_ref(address).cast(),
                size_of::<SockAddrL2>() as libc::socklen_t,
            )
        })
        .map(drop)
    }

    /// The asynchronous result of a connect that returned `EINPROGRESS`.
    pub(super) fn take_error(fd: &OwnedFd) -> io::Result<Option<io::Error>> {
        let mut error: libc::c_int = 0;
        let mut length = size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: `error` and `length` are valid for writes of the sizes passed.
        check(unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&raw mut error).cast(),
                &raw mut length,
            )
        })?;
        Ok((error != 0).then(|| io::Error::from_raw_os_error(error)))
    }

    pub(super) fn recv(fd: &OwnedFd, buffer: &mut [u8]) -> io::Result<usize> {
        // SAFETY: the pointer and length describe `buffer`, which is valid for writes.
        let received =
            unsafe { libc::recv(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len(), 0) };
        usize::try_from(received).map_err(|_negative| io::Error::last_os_error())
    }

    pub(super) fn send(fd: &OwnedFd, buffer: &[u8]) -> io::Result<usize> {
        // SAFETY: the pointer and length describe `buffer`, which is valid for reads.
        let sent = unsafe {
            libc::send(
                fd.as_raw_fd(),
                buffer.as_ptr().cast(),
                buffer.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        usize::try_from(sent).map_err(|_negative| io::Error::last_os_error())
    }
}

/// One connected L2CAP channel. Dropping the last reference closes it.
#[derive(Debug)]
struct Socket(AsyncFd<std::os::fd::OwnedFd>);

impl Socket {
    async fn connect(address: [u8; 6], channel: Channel) -> io::Result<Self> {
        let fd = sys::socket()?;
        sys::bind(&fd, &SockAddrL2::new(0, BDADDR_ANY, BDADDR_LE_PUBLIC))?;
        sys::require_medium_security(&fd)?;
        let socket = AsyncFd::with_interest(fd, Interest::READABLE | Interest::WRITABLE)?;
        let peer = SockAddrL2::new(channel.psm(), address, BDADDR_LE_RANDOM);
        match sys::connect(socket.get_ref(), &peer) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::EINPROGRESS) => {
                // Writable once the channel is open or the attempt failed. The guard
                // is dropped without clearing, so the first send does not wait again.
                drop(socket.writable().await?);
                if let Some(e) = sys::take_error(socket.get_ref())? {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
        Ok(Self(socket))
    }

    async fn recv(&self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0
            .async_io(Interest::READABLE, |fd| sys::recv(fd, buffer))
            .await
    }

    async fn send(&self, sdu: &[u8]) -> io::Result<usize> {
        self.0
            .async_io(Interest::WRITABLE, |fd| sys::send(fd, sdu))
            .await
    }
}

/// Both channels of an established link.
#[derive(Debug)]
pub(super) struct L2capLink {
    control: Socket,
    data: Socket,
}

/// Opens the control channel, then the data channel (the unit accepts them in that
/// order). Dropping the result closes the link.
pub(super) async fn connect(address: [u8; 6]) -> Result<L2capLink> {
    let open = async |channel| {
        tokio::time::timeout(CONNECT_TIMEOUT, Socket::connect(address, channel))
            .await
            .map_err(|_elapsed| Error::Bluetooth(io::ErrorKind::TimedOut.into()))?
            .map_err(Error::Bluetooth)
    };
    let control = open(Channel::Control).await?;
    let data = open(Channel::Data).await?;
    debug!("l2cap: both channels open to {address:02x?}");
    Ok(L2capLink { control, data })
}

/// Starts the reader and writer of `link`. Both end, and close the sockets, when
/// `cancel` fires, when the unit closes a channel, or on an I/O error, and then
/// cancel `cancel` themselves.
pub(super) fn spawn(
    link: L2capLink,
    writer_rx: mpsc::Receiver<Packet>,
    incoming_tx: mpsc::Sender<Packet>,
    cancel: &CancellationToken,
) {
    let L2capLink { control, data } = link;
    let (control, data) = (Arc::new(control), Arc::new(data));
    tokio::spawn(write_packets(
        Arc::clone(&control),
        Arc::clone(&data),
        writer_rx,
        cancel.clone(),
    ));
    tokio::spawn(read_packets(control, data, incoming_tx, cancel.clone()));
}

async fn read_packets(
    control: Arc<Socket>,
    data: Arc<Socket>,
    incoming_tx: mpsc::Sender<Packet>,
    cancel: CancellationToken,
) {
    let mut control_buffer = vec![0; RECV_BUFFER_SIZE];
    let mut data_buffer = vec![0; RECV_BUFFER_SIZE];
    loop {
        let (channel, received) = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            () = incoming_tx.closed() => break,
            received = control.recv(&mut control_buffer) => (Channel::Control, received),
            received = data.recv(&mut data_buffer) => (Channel::Data, received),
        };
        let length = match received {
            // A zero-length read is the kernel's end of file; the unit sends no empty SDUs.
            Ok(0) => {
                info!("l2cap {channel:?} channel closed by the unit");
                break;
            }
            Ok(length) => length,
            Err(e) => {
                warn!("l2cap {channel:?} receive failed: {e}");
                break;
            }
        };
        let sdu = match channel {
            Channel::Control => &control_buffer[..length],
            Channel::Data => &data_buffer[..length],
        };
        let packet = match postcard::from_bytes::<Packet>(sdu) {
            Ok(packet) => packet,
            Err(e) => {
                warn!("l2cap {channel:?} SDU of {length} bytes is not a packet: {e}");
                continue;
            }
        };
        tokio::select! {
            () = cancel.cancelled() => break,
            result = incoming_tx.send(packet) => if result.is_err() { break; },
        }
    }
    cancel.cancel();
    info!("l2cap reader exits");
}

async fn write_packets(
    control: Arc<Socket>,
    data: Arc<Socket>,
    mut writer_rx: mpsc::Receiver<Packet>,
    cancel: CancellationToken,
) {
    loop {
        let packet = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            packet = writer_rx.recv() => match packet {
                Some(packet) => packet,
                None => break,
            },
        };
        let channel = Channel::for_packet(&packet);
        let sdu = match postcard::to_stdvec(&packet) {
            Ok(sdu) => sdu,
            Err(e) => {
                error!("l2cap packet serialize failed: {e}");
                continue;
            }
        };
        let socket = match channel {
            Channel::Control => &control,
            Channel::Data => &data,
        };
        let result = tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            result = socket.send(&sdu) => result,
        };
        match result {
            Ok(_) => {}
            // Larger than the unit's MTU: this packet can never be sent, the link is fine.
            Err(e) if e.raw_os_error() == Some(libc::EMSGSIZE) => {
                warn!(
                    "l2cap {channel:?} packet of {} bytes exceeds the MTU",
                    sdu.len()
                );
            }
            Err(e) => {
                error!("l2cap {channel:?} send failed: {e}");
                break;
            }
        }
    }
    cancel.cancel();
    info!("l2cap writer exits");
}

#[cfg(test)]
mod tests {
    use std::mem::offset_of;

    use super::*;
    use crate::packets::vm::{
        ImpactReport, PacketData, PacketType, StreamUpdate, StreamUpdateAction,
    };

    #[test]
    fn sockaddr_l2_matches_the_kernel_layout() {
        assert_eq!(size_of::<SockAddrL2>(), 14);
        assert_eq!(offset_of!(SockAddrL2, l2_family), 0);
        assert_eq!(offset_of!(SockAddrL2, l2_psm), 2);
        assert_eq!(offset_of!(SockAddrL2, l2_bdaddr), 4);
        assert_eq!(offset_of!(SockAddrL2, l2_cid), 10);
        assert_eq!(offset_of!(SockAddrL2, l2_bdaddr_type), 12);
    }

    #[test]
    fn sockaddr_keeps_address_order_and_stores_psm_little_endian() {
        let address = [0x01, 0x02, 0x03, 0x04, 0x05, 0xC6];
        let sockaddr = SockAddrL2::new(PSM_CONTROL, address, BDADDR_LE_RANDOM);
        assert_eq!(sockaddr.l2_bdaddr, address);
        assert_eq!(sockaddr.l2_psm.to_ne_bytes(), [0x80, 0x00]);
        assert_eq!(sockaddr.l2_cid, 0);
        assert_eq!(sockaddr.l2_bdaddr_type, BDADDR_LE_RANDOM);
        assert_eq!(i32::from(sockaddr.l2_family), libc::AF_BLUETOOTH);
    }

    #[test]
    fn commands_take_the_control_channel_and_reports_the_data_channel() {
        let command = Packet {
            id: 1,
            data: PacketData::StreamUpdate(StreamUpdate {
                packet_id: PacketType::End(),
                action: StreamUpdateAction::DisableAll,
            }),
        };
        let report = Packet {
            id: 0,
            data: PacketData::ImpactReport(ImpactReport { timestamp: 0 }),
        };
        assert_eq!(Channel::for_packet(&command), Channel::Control);
        assert_eq!(Channel::for_packet(&report), Channel::Data);
        assert_eq!(Channel::Control.psm(), 0x0080);
        assert_eq!(Channel::Data.psm(), 0x0081);
    }

    #[test]
    fn one_sdu_is_one_bare_postcard_packet() {
        let packet = Packet {
            id: 7,
            data: PacketData::ReadVersion(),
        };
        let sdu = postcard::to_stdvec(&packet).expect("a version request encodes");
        let decoded: Packet = postcard::from_bytes(&sdu).expect("the SDU decodes");
        assert_eq!(decoded.id, 7);
        assert!(matches!(decoded.data, PacketData::ReadVersion()));
    }
}
