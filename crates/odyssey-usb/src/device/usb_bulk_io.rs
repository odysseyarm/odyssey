//! The bulk endpoint pair both the vision module and the dongle expose, and
//! the reader and writer loops that run over it.

use std::io::ErrorKind;

use nusb::io::{EndpointRead, EndpointWrite};
use nusb::transfer::{Bulk, In, Out};
use nusb::{DeviceInfo, Interface};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::{Error, Result};

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
/// Initial capacity of the encode buffer. Most messages are far smaller.
const WRITE_SCRATCH_CAPACITY: usize = 1024;
/// Initial capacity of the receive buffer for one message.
const READ_SCRATCH_CAPACITY: usize = 256;
/// Packets and mux envelopes are bounded protocol types, and even a worst-case
/// object report is below 1 KiB. Refuse an unterminated transfer before it can
/// grow the host's receive buffer without bound.
const MAX_MESSAGE_SIZE: usize = 4096;
/// Unclassified read errors tolerated in a row before the reader gives up.
const MAX_CONSECUTIVE_READ_ERRORS: u8 = 3;

/// Number of the vendor-specific interface, class `0xFF`.
pub(super) fn find_vendor_interface(info: &DeviceInfo) -> Result<u8> {
    info.interfaces()
        .find(|iface| iface.class() == VENDOR_SPECIFIC_CLASS)
        .map(nusb::InterfaceInfo::interface_number)
        .ok_or(Error::NoVendorInterface)
}

/// Opens the bulk IN and OUT endpoints of a claimed interface.
pub(super) fn open_bulk_endpoints(
    iface: &Interface,
) -> Result<(EndpointRead<Bulk>, EndpointWrite<Bulk>)> {
    let in_ep = iface
        .endpoint::<Bulk, In>(BULK_IN_ENDPOINT)
        .map_err(|source| Error::Endpoint {
            address: BULK_IN_ENDPOINT,
            source,
        })?
        .reader(ENDPOINT_BUFFER_SIZE);
    let out_ep = iface
        .endpoint::<Bulk, Out>(BULK_OUT_ENDPOINT)
        .map_err(|source| Error::Endpoint {
            address: BULK_OUT_ENDPOINT,
            source,
        })?
        .writer(ENDPOINT_BUFFER_SIZE);
    Ok((in_ep, out_ep))
}

/// Encodes each message from `outgoing`, ending it with a USB short packet or ZLP.
///
/// A message that fails to encode is logged and dropped. A failed write ends
/// the link: its partially written frame cannot safely precede another one.
pub(super) fn spawn_writer<M>(
    mut out_ep: EndpointWrite<Bulk>,
    mut outgoing: mpsc::Receiver<M>,
    label: &'static str,
    cancel: CancellationToken,
) where
    M: Serialize + Send + 'static,
{
    tokio::spawn(async move {
        let mut raw = Vec::with_capacity(WRITE_SCRATCH_CAPACITY);
        loop {
            let msg = tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                message = outgoing.recv() => match message {
                    Some(message) => message,
                    None => break,
                },
            };
            raw.clear();
            if let Err(e) = postcard::to_io(&msg, &mut raw) {
                error!("{label} postcard serialize failed: {e}");
                continue;
            }
            let result = tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                result = async {
                    out_ep.write_all(&raw).await?;
                    out_ep.flush_end_async().await
                } => result,
            };
            if let Err(e) = result {
                error!("{label} bulk write failed: {e}");
                break;
            }
        }
        cancel.cancel();
        info!("{label} writer exits");
    });
}

/// What a failed read means for the reader loop.
#[derive(Debug, PartialEq, Eq)]
enum ReadErrorAction {
    /// Nothing arrived in time. Poll again.
    Idle,
    /// A glitch within budget. Log it and poll again.
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

/// Reads postcard records from bulk IN transfers, including multiple records
/// carried by one USB transfer.
pub(super) struct MessageReader {
    in_ep: EndpointRead<Bulk>,
    budget: ReadErrorBudget,
    label: &'static str,
    buffer: Vec<u8>,
    position: usize,
}

impl MessageReader {
    pub(super) fn new(in_ep: EndpointRead<Bulk>, label: &'static str) -> Self {
        Self {
            in_ep,
            budget: ReadErrorBudget::default(),
            label,
            buffer: Vec::with_capacity(READ_SCRATCH_CAPACITY),
            position: 0,
        }
    }

    fn decode_buffer<M: DeserializeOwned>(
        buffer: &[u8],
        position: &mut usize,
    ) -> std::result::Result<Option<M>, postcard::Error> {
        if *position == buffer.len() {
            return Ok(None);
        }
        match postcard::take_from_bytes::<M>(&buffer[*position..]) {
            Ok((message, remaining)) => {
                *position = buffer.len() - remaining.len();
                Ok(Some(message))
            }
            Err(error) => {
                *position = buffer.len();
                Err(error)
            }
        }
    }

    /// Next postcard record, or `None` once link has failed.
    pub(super) async fn next_message<M: DeserializeOwned>(&mut self) -> Option<M> {
        let label = self.label;
        loop {
            match Self::decode_buffer(&self.buffer, &mut self.position) {
                Ok(Some(message)) => return Some(message),
                Ok(None) => {}
                Err(error) => error!("{label} postcard decode failed: {error:?}"),
            }
            self.buffer.clear();
            self.position = 0;
            let mut reader = self.in_ep.until_short_packet();
            match (&mut reader)
                .take((MAX_MESSAGE_SIZE + 1) as u64)
                .read_to_end(&mut self.buffer)
                .await
            {
                Ok(_) => self.budget.on_success(),
                Err(e) => {
                    self.buffer.clear();
                    match self.budget.on_error(e.kind()) {
                        ReadErrorAction::Idle => continue,
                        ReadErrorAction::Tolerate => {
                            warn!("{label} read error (ignored): {e}");
                            continue;
                        }
                        ReadErrorAction::GiveUp => {
                            error!("{label} read error: {e:?} (kind: {:?})", e.kind());
                            return None;
                        }
                    }
                }
            }
            if self.buffer.len() > MAX_MESSAGE_SIZE {
                error!("{label} transfer exceeds {MAX_MESSAGE_SIZE} bytes");
                return None;
            }
            if let Err(e) = reader.consume_end() {
                warn!("{label} consume_end failed: {e:?}");
                self.buffer.clear();
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
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum Record {
        Marker(u8),
        Accel(u16),
    }

    #[test]
    fn decodes_multiple_mixed_records_in_order() {
        let records = [Record::Marker(3), Record::Accel(8), Record::Marker(5)];
        let transfer: Vec<u8> = records
            .iter()
            .flat_map(|record| postcard::to_stdvec(record).unwrap())
            .collect();

        let mut position = 0;
        for expected in records {
            assert_eq!(
                MessageReader::decode_buffer::<Record>(&transfer, &mut position).unwrap(),
                Some(expected)
            );
        }
        assert_eq!(
            MessageReader::decode_buffer::<Record>(&transfer, &mut position).unwrap(),
            None
        );
    }

    #[test]
    fn malformed_or_truncated_trailing_record_is_dropped_before_next_transfer() {
        let first = Record::Marker(3);
        let second = Record::Accel(8);
        let mut transfer = postcard::to_stdvec(&first).unwrap();
        transfer.extend([0x01, 0x80]);
        let mut position = 0;
        assert_eq!(
            MessageReader::decode_buffer::<Record>(&transfer, &mut position).unwrap(),
            Some(first)
        );
        MessageReader::decode_buffer::<Record>(&transfer, &mut position)
            .expect_err("truncated u16");
        assert_eq!(
            MessageReader::decode_buffer::<Record>(&transfer, &mut position).unwrap(),
            None
        );
        transfer = postcard::to_stdvec(&second).unwrap();
        position = 0;
        assert_eq!(
            MessageReader::decode_buffer::<Record>(&transfer, &mut position).unwrap(),
            Some(second)
        );
    }
}
