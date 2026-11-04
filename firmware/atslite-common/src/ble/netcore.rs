//! Net-core release query.
//!
//! The net core answers the vendor HCI command `0xFFF0` itself with its release counter. A net core
//! without that feature forwards the opcode to the SoftDevice Controller, which answers Command
//! Complete with status 0x01 (unknown command) and no return parameters; the host reports that as
//! "unknown" (0).

// The bt-hci macros test their own `defmt` feature name, which this crate does not declare.
#![allow(unexpected_cfgs)]

use core::sync::atomic::{AtomicU16, Ordering};

use backon::{EmbassySleeper, ExponentialBuilder, Retryable};
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

/// Pause before the first query, so the net core has a moment to start answering.
const INITIAL_DELAY_MS: u64 = 100;
/// Delay before the second query; each later retry waits twice as long as the one before. The
/// retries together give the net core a few seconds to start answering before giving up.
const RETRY_MIN_DELAY: core::time::Duration = core::time::Duration::from_millis(250);
/// Queries made in total, the first included.
const MAX_ATTEMPTS: usize = 5;
/// `Stack::command` first waits for the host to finish initializing, so this covers that too.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(3);

enum QueryError {
    /// The controller answered but does not implement the command; retrying cannot help.
    Unavailable(bt_hci::param::Error),
    /// The query failed or timed out; the net core may still be starting.
    Transient,
}

/// Query the net-core release once the host runner is being polled, and store it for
/// [`netcore_release`]. Retries with backoff because the net core may still be starting; gives up
/// quietly so BLE and USB never depend on the answer.
pub async fn query<C, P>(stack: &Stack<'_, C, P>)
where
    C: Controller + ControllerCmdSync<ReadNetCoreRelease>,
    P: PacketPool,
{
    Timer::after_millis(INITIAL_DELAY_MS).await;
    let mut attempts = 0usize;
    let result = (|| {
        let attempt = attempts;
        attempts += 1;
        async move {
            match with_timeout(ATTEMPT_TIMEOUT, stack.command(ReadNetCoreRelease::new())).await {
                Ok(Ok(ret)) => Ok(ret),
                Ok(Err(trouble_host::BleHostError::BleHost(trouble_host::Error::Hci(e)))) => {
                    Err(QueryError::Unavailable(e))
                }
                Ok(Err(_)) => {
                    defmt::warn!("[ble] net-core release query failed (attempt {})", attempt);
                    Err(QueryError::Transient)
                }
                Err(_) => {
                    defmt::warn!(
                        "[ble] net-core release query timed out (attempt {})",
                        attempt
                    );
                    Err(QueryError::Transient)
                }
            }
        }
    })
    .retry(
        ExponentialBuilder::default()
            .with_min_delay(RETRY_MIN_DELAY)
            .with_factor(2.0)
            .with_max_times(MAX_ATTEMPTS - 1),
    )
    .sleep(EmbassySleeper)
    .when(|e| matches!(e, QueryError::Transient))
    .await;
    match result {
        Ok(ret) => {
            let release = { ret.release };
            NETCORE_RELEASE.store(release, Ordering::Relaxed);
            defmt::info!("[ble] net-core release {}", release);
        }
        Err(QueryError::Unavailable(e)) => {
            defmt::info!("[ble] net-core release unavailable: {:?}", e);
        }
        Err(QueryError::Transient) => {}
    }
}
