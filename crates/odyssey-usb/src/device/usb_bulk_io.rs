//! The bulk endpoint pair both the vision module and the dongle expose, and
//! the reader and writer loops that run over it.

use std::io::ErrorKind;

use anyhow::{Result, anyhow};
use nusb::io::{EndpointRead, EndpointWrite};
use nusb::transfer::{Bulk, In, Out};
use nusb::{DeviceInfo, Interface};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Bulk IN endpoint address (direction bit set).
const BULK_IN_ENDPOINT: u8 = 0x81;
/// Bulk OUT endpoint address.
const BULK_OUT_ENDPOINT: u8 = 0x01;
/// Buffering of each endpoint adapter, several full-speed packets deep so a
/// burst of reports does not wait on the scheduler.
const ENDPOINT_BUFFER_SIZE: usize = 4096;
/// USB interface class of the vendor-specific interface that carries the
/// protocol.
const VENDOR_SPECIFIC_CLASS: u8 = 0xFF;
/// Initial capacity of the encode buffer; most messages are far smaller.
const WRITE_SCRATCH_CAPACITY: usize = 1024;
/// Initial capacity of the receive buffer for one message.
const READ_SCRATCH_CAPACITY: usize = 256;
/// Unclassified read errors tolerated in a row before the reader gives up.
const MAX_CONSECUTIVE_READ_ERRORS: u8 = 3;

/// Number of the vendor-specific interface, class `0xFF`.
pub(super) fn find_vendor_interface(info: &DeviceInfo) -> Result<u8> {
    info.interfaces()
        .find(|iface| iface.class() == VENDOR_SPECIFIC_CLASS)
        .map(nusb::InterfaceInfo::interface_number)
        .ok_or_else(|| anyhow!("No vendor interface (class 0xFF) found"))
}

/// Opens the bulk IN and OUT endpoints of a claimed interface.
pub(super) fn open_bulk_endpoints(
    iface: &Interface,
) -> Result<(EndpointRead<Bulk>, EndpointWrite<Bulk>)> {
    let in_ep = iface
        .endpoint::<Bulk, In>(BULK_IN_ENDPOINT)
        .map_err(|e| anyhow!("Failed to get IN endpoint {BULK_IN_ENDPOINT:#04x}: {e}"))?
        .reader(ENDPOINT_BUFFER_SIZE);
    let out_ep = iface
        .endpoint::<Bulk, Out>(BULK_OUT_ENDPOINT)
        .map_err(|e| anyhow!("Failed to get OUT endpoint {BULK_OUT_ENDPOINT:#04x}: {e}"))?
        .writer(ENDPOINT_BUFFER_SIZE);
    Ok((in_ep, out_ep))
}

/// Encodes each message from `outgoing` and writes it as one USB transfer.
///
/// A message that fails to encode or write is logged and dropped; the task
/// ends when every sender is gone. `label` names the link in the log.
pub(super) fn spawn_writer<M>(
    mut out_ep: EndpointWrite<Bulk>,
    mut outgoing: mpsc::Receiver<M>,
    label: &'static str,
) where
    M: Serialize + Send + 'static,
{
    tokio::spawn(async move {
        let mut raw = Vec::with_capacity(WRITE_SCRATCH_CAPACITY);
        while let Some(msg) = outgoing.recv().await {
            raw.clear();
            if let Err(e) = postcard::to_io(&msg, &mut raw) {
                error!("{label} postcard serialize failed: {e}");
                continue;
            }
            if let Err(e) = out_ep.write_all(&raw).await {
                error!("{label} write_all failed: {e}");
                continue;
            }
            if let Err(e) = out_ep.flush_end_async().await {
                error!("{label} flush_end failed: {e:?}");
            }
        }
        info!("{label} writer exits");
    });
}

/// What a failed read means for the reader loop.
#[derive(Debug, PartialEq, Eq)]
enum ReadErrorAction {
    /// Nothing arrived in time; poll again.
    Idle,
    /// A glitch within budget; log it and poll again.
    Tolerate,
    /// The link is gone, or the glitches did not stop.
    GiveUp,
}

/// Counts consecutive unclassified read errors against
/// [`MAX_CONSECUTIVE_READ_ERRORS`].
#[derive(Debug, Default)]
struct ReadErrorBudget {
    consecutive: u8,
}

impl ReadErrorBudget {
    fn on_success(&mut self) {
        self.consecutive = 0;
    }

    fn on_error(&mut self, kind: ErrorKind) -> ReadErrorAction {
        match kind {
            ErrorKind::TimedOut | ErrorKind::WouldBlock => {
                self.consecutive = 0;
                ReadErrorAction::Idle
            }
            ErrorKind::BrokenPipe | ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset => {
                ReadErrorAction::GiveUp
            }
            _ if self.consecutive < MAX_CONSECUTIVE_READ_ERRORS => {
                self.consecutive += 1;
                ReadErrorAction::Tolerate
            }
            _ => ReadErrorAction::GiveUp,
        }
    }
}

/// Reads whole messages off the bulk IN endpoint, one per short packet.
pub(super) struct MessageReader {
    in_ep: EndpointRead<Bulk>,
    budget: ReadErrorBudget,
    label: &'static str,
}

impl MessageReader {
    pub(super) fn new(in_ep: EndpointRead<Bulk>, label: &'static str) -> Self {
        Self {
            in_ep,
            budget: ReadErrorBudget::default(),
            label,
        }
    }

    /// The next decodable message, or `None` once the link has failed.
    /// A message that does not decode is logged and skipped.
    pub(super) async fn next_message<M: DeserializeOwned>(&mut self) -> Option<M> {
        let label = self.label;
        let mut reader = self.in_ep.until_short_packet();
        loop {
            let mut buf = Vec::with_capacity(READ_SCRATCH_CAPACITY);
            match reader.read_to_end(&mut buf).await {
                Ok(_) => self.budget.on_success(),
                Err(e) => match self.budget.on_error(e.kind()) {
                    ReadErrorAction::Idle => continue,
                    ReadErrorAction::Tolerate => {
                        warn!("{label} read error (ignored): {e}");
                        continue;
                    }
                    ReadErrorAction::GiveUp => {
                        error!("{label} read error: {e:?} (kind: {:?})", e.kind());
                        return None;
                    }
                },
            }
            if let Err(e) = reader.consume_end() {
                warn!("{label} consume_end failed: {e:?}");
                continue;
            }
            match postcard::from_bytes::<M>(&buf) {
                Ok(msg) => return Some(msg),
                Err(e) => error!("{label} postcard decode failed: {e:?}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeouts_are_idle_and_reset_the_error_count() {
        let mut budget = ReadErrorBudget::default();
        assert_eq!(budget.on_error(ErrorKind::Other), ReadErrorAction::Tolerate);
        assert_eq!(budget.on_error(ErrorKind::TimedOut), ReadErrorAction::Idle);
        assert_eq!(budget.consecutive, 0);
    }

    #[test]
    fn a_vanished_device_ends_the_reader_at_once() {
        for kind in [
            ErrorKind::BrokenPipe,
            ErrorKind::UnexpectedEof,
            ErrorKind::ConnectionReset,
        ] {
            assert_eq!(
                ReadErrorBudget::default().on_error(kind),
                ReadErrorAction::GiveUp
            );
        }
    }

    #[test]
    fn other_errors_are_tolerated_three_times_then_fatal() {
        let mut budget = ReadErrorBudget::default();
        for _ in 0..MAX_CONSECUTIVE_READ_ERRORS {
            assert_eq!(budget.on_error(ErrorKind::Other), ReadErrorAction::Tolerate);
        }
        assert_eq!(budget.on_error(ErrorKind::Other), ReadErrorAction::GiveUp);
    }

    #[test]
    fn a_good_read_clears_the_error_streak() {
        let mut budget = ReadErrorBudget::default();
        for _ in 0..MAX_CONSECUTIVE_READ_ERRORS {
            budget.on_error(ErrorKind::Other);
        }
        budget.on_success();
        assert_eq!(budget.on_error(ErrorKind::Other), ReadErrorAction::Tolerate);
    }
}
