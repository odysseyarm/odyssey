//! Net-core release query.
//!
//! The net core answers the vendor HCI command `0xFFF0` itself with its release counter. A net core
//! without that feature forwards the opcode to the SoftDevice Controller, which answers Command
//! Complete with status 0x01 (unknown command) and no return parameters; the host reports that as
//! "unknown" (0).

// The bt-hci macros test their own `defmt` feature name, which this crate does not declare.
#![allow(unexpected_cfgs)]

use core::sync::atomic::{AtomicU16, Ordering};

use bt_hci::cmd;
use bt_hci::controller::ControllerCmdSync;
use embassy_time::{Duration, Timer, with_timeout};
use trouble_host::prelude::{Controller, PacketPool, Stack};

cmd! {
    /// Vendor command (OGF 0x3F, OCF 0x3F0) that returns the net-core release counter.
    ReadNetCoreRelease(VENDOR_SPECIFIC, 0x3F0) {
        Params = ();
        ReadNetCoreReleaseReturn {
            release: u16,
        }
    }
}

/// Release counter reported by the net core; 0 means unknown.
static NETCORE_RELEASE: AtomicU16 = AtomicU16::new(0);

/// Release counter of the net-core firmware, or 0 when the net core did not report one (older
/// net-core firmware, or not queried yet).
pub fn netcore_release() -> u16 {
    NETCORE_RELEASE.load(Ordering::Relaxed)
}

/// Gives the net core a few seconds to start answering before giving up.
const BACKOFF_MS: [u64; 5] = [100, 250, 500, 1000, 2000];
/// `Stack::command` first waits for the host to finish initializing, so this covers that too.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(3);

/// Query the net-core release once the host runner is being polled, and store it for
/// [`netcore_release`]. Retries with backoff because the net core may still be starting; gives up
/// quietly so BLE and USB never depend on the answer.
pub async fn query<C, P>(stack: &Stack<'_, C, P>)
where
    C: Controller + ControllerCmdSync<ReadNetCoreRelease>,
    P: PacketPool,
{
    for (attempt, backoff) in BACKOFF_MS.iter().enumerate() {
        Timer::after_millis(*backoff).await;
        match with_timeout(ATTEMPT_TIMEOUT, stack.command(ReadNetCoreRelease::new())).await {
            Ok(Ok(ret)) => {
                let release = { ret.release };
                NETCORE_RELEASE.store(release, Ordering::Relaxed);
                defmt::info!("[ble] net-core release {}", release);
                return;
            }
            Ok(Err(trouble_host::BleHostError::BleHost(trouble_host::Error::Hci(e)))) => {
                // The controller answered but does not implement the command.
                defmt::info!("[ble] net-core release unavailable: {:?}", e);
                return;
            }
            Ok(Err(_)) => {
                defmt::warn!("[ble] net-core release query failed (attempt {})", attempt);
            }
            Err(_) => {
                defmt::warn!(
                    "[ble] net-core release query timed out (attempt {})",
                    attempt
                );
            }
        }
    }
}
