//! Vendor control transfers on EP0 (interface recipient), the side channel a
//! device and the dongle expose next to their bulk endpoints.
//!
//! A message is postcard-encoded and pushed with a vendor `SEND` request.
//! Replies and unsolicited events cannot be pushed to the host, so the host
//! polls with a vendor `RECV` request that returns one encoded message, or
//! nothing when none is queued.

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use nusb::Interface;
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient, TransferError};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Vendor request that delivers a control message to the device.
const REQUEST_SEND: u8 = 0x30;
/// Vendor request that returns the next queued control message, if any.
const REQUEST_RECV: u8 = 0x31;
/// A device that cannot take a control message within this has stalled.
const SEND_TIMEOUT: Duration = Duration::from_millis(1000);
/// Length of one poll. A poll that finds nothing is cancelled by the host
/// after this, so a long wait is many short polls.
const POLL_SLICE: Duration = Duration::from_millis(500);
/// Largest control message the host asks for in one poll.
const RECV_BUFFER_LENGTH: u16 = 256;

/// How long to wait for the reply to an ordinary control request.
pub(super) const REPLY_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait for a reply that follows a flash write.
pub(super) const FLASH_WRITE_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// What to do with a reply that does not decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UndecodableReply {
    /// Keep polling: it may be an event from firmware newer than this host.
    /// Fails only if the deadline has already passed.
    Skip,
    /// Fail at once.
    Fail,
}

pub(super) async fn send<M: Serialize>(iface: &Interface, msg: &M) -> Result<()> {
    let data = postcard::to_allocvec(msg).map_err(|e| anyhow!("ctrl encode failed: {e}"))?;
    iface
        .control_out(
            ControlOut {
                control_type: ControlType::Vendor,
                recipient: Recipient::Interface,
                request: REQUEST_SEND,
                value: 0,
                index: u16::from(iface.interface_number()),
                data: &data,
            },
            SEND_TIMEOUT,
        )
        .await
        .map_err(|e| anyhow!("ctrl send failed: {e}"))
}

/// Polls until a message satisfying `accept` arrives, discarding the others.
pub(super) async fn recv_matching<M: DeserializeOwned>(
    iface: &Interface,
    timeout: Duration,
    undecodable: UndecodableReply,
    mut accept: impl FnMut(&M) -> bool,
) -> Result<M> {
    let index = u16::from(iface.interface_number());
    let start = Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            return Err(anyhow!("timeout waiting for ctrl event"));
        }
        let slice = (timeout - elapsed).min(POLL_SLICE);
        let reply = match iface
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Interface,
                    request: REQUEST_RECV,
                    value: 0,
                    index,
                    length: RECV_BUFFER_LENGTH,
                },
                slice,
            )
            .await
        {
            Ok(data) => data,
            // The poll slice ran out with nothing queued.
            Err(TransferError::Cancelled) => continue,
            Err(e) => return Err(anyhow!("ctrl_recv control_in failed: {e:?}")),
        };
        if reply.is_empty() {
            continue;
        }

        let msg = match postcard::from_bytes::<M>(&reply) {
            Ok(msg) => msg,
            Err(e) => {
                if undecodable == UndecodableReply::Skip && start.elapsed() < timeout {
                    continue;
                }
                return Err(anyhow!("ctrl decode failed: {e}"));
            }
        };
        if accept(&msg) {
            return Ok(msg);
        }
    }
}
