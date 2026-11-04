use std::time::Duration;

use anyhow::Result;
use backon::Retryable as _;
use odyssey_usb::device::VmDevice;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::debug;

use super::messages::DeviceTaskMessage;
use super::semver::semver_at_least;

/// First device protocol that carries `PropKind::NetCoreRelease`. Older
/// applications cannot decode the request.
const NETCORE_RELEASE_PROTOCOL: [u16; 3] = [0, 1, 5];

/// How a net core release read reaches the application.
pub(super) enum NetCoreLink {
    /// USB control endpoint: a unit on the cable.
    Control(VmDevice),
    /// The packet stream: a unit reached over Bluetooth through a dongle.
    Packets(VmDevice),
}

impl NetCoreLink {
    async fn read(&self) -> Result<u16> {
        match self {
            Self::Control(device) => device.read_netcore_release().await,
            Self::Packets(device) => device.read_netcore_release_over_packets().await,
        }
    }
}

/// Gaps before each net core read.
pub(super) const NETCORE_PROBE_DELAYS_S: [u64; 6] = [0, 1, 2, 4, 8, 15];

/// Poll `read` for the net core release; returns the release to report to the
/// handler, or `None` to leave it unknown.
///
/// The application asks the net core itself once its BLE host is running and
/// keeps asking while the net core is still starting, so a read right after
/// connecting can legitimately come back 0. The probe retries with growing
/// gaps, and gives up for good after the last one: polling forever would only
/// compete with the handler for the link. A failed or unanswered read never
/// affects the device itself and leaves the release unknown.
///
/// A 0 is only reported (as settled) when `settle_zero` and the *final* read
/// itself answered 0: a net core that still says 0 after every retry has no
/// release to report. An error or timeout after a 0 clears that, because the
/// last word from the unit is then "no answer", not "0".
async fn probe_net_core<F, Fut>(
    delays_s: &[u64],
    settle_zero: bool,
    is_closed: impl Fn() -> bool,
    mut read: F,
) -> Option<u16>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<u16>>,
{
    let (first_delay_s, later_delays_s) = delays_s.split_first()?;
    sleep(Duration::from_secs(*first_delay_s)).await;
    let outcome = (|| {
        let pending = (!is_closed()).then(&mut read);
        async {
            let Some(pending) = pending else {
                return Err(ProbeMiss::Closed);
            };
            match tokio::time::timeout(Duration::from_secs(3), pending).await {
                Ok(Ok(0)) => {
                    debug!("Net core release not reported yet");
                    Err(ProbeMiss::Zero)
                }
                Ok(Ok(release)) => Ok(release),
                Ok(Err(e)) => {
                    debug!("Could not read the net core release: {e}");
                    Err(ProbeMiss::NoAnswer)
                }
                Err(_) => {
                    debug!("Reading the net core release timed out");
                    Err(ProbeMiss::NoAnswer)
                }
            }
        }
    })
    .retry(later_delays_s.iter().map(|&s| Duration::from_secs(s)))
    .when(|miss| !matches!(miss, ProbeMiss::Closed))
    .await;
    match outcome {
        Ok(release) => Some(release),
        Err(ProbeMiss::Zero) => settle_zero.then_some(0),
        Err(ProbeMiss::Closed | ProbeMiss::NoAnswer) => None,
    }
}

/// Why one net core read produced no release.
enum ProbeMiss {
    /// The device went away: stop probing.
    Closed,
    /// The net core answered 0: it has not reported a release yet.
    Zero,
    /// The read failed or timed out.
    NoAnswer,
}

/// Read the net core release over `link` and hand it to the device's handler
/// task. See [`probe_net_core`] for when a reading counts.
///
/// Over the cable a settled 0 is real (the net core predates release
/// reporting); over Bluetooth a 0 is also what an unanswered read looks like,
/// so only a positive release is reported there. A device protocol that cannot
/// carry the property never starts a probe and stays unknown.
pub(super) fn spawn_netcore_probe(
    link: NetCoreLink,
    device_protocol: Option<[u16; 3]>,
    handler: mpsc::Sender<DeviceTaskMessage>,
) {
    if !device_protocol.is_some_and(|p| semver_at_least(p, NETCORE_RELEASE_PROTOCOL)) {
        return;
    }
    tokio::spawn(async move {
        let over_control = matches!(link, NetCoreLink::Control(_));
        let release = probe_net_core(
            &NETCORE_PROBE_DELAYS_S,
            over_control,
            || handler.is_closed(),
            || link.read(),
        )
        .await;
        if let Some(release) = release {
            let _ = handler
                .send(DeviceTaskMessage::SetNetCoreRelease(release))
                .await;
        }
    });
}

/// Starts a probe over the packet stream for a unit whose only link is
/// Bluetooth. The handler passes a weak sender so that holding it does not keep
/// its own channel open after every other owner has dropped theirs.
pub(super) fn spawn_packet_netcore_probe(
    handler: &mpsc::WeakSender<DeviceTaskMessage>,
    events: &VmDevice,
    device_protocol: [u16; 3],
) {
    if let Some(handler) = handler.upgrade() {
        spawn_netcore_probe(
            NetCoreLink::Packets(events.clone()),
            Some(device_protocol),
            handler,
        );
    }
}

#[cfg(test)]
mod netcore_probe_tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;

    /// Runs the probe against a scripted unit; the script must cover every read.
    async fn probe(script: Vec<Result<u16>>, settle_zero: bool) -> Option<u16> {
        let delays = vec![0u64; script.len()];
        let script = RefCell::new(VecDeque::from(script));
        probe_net_core(
            &delays,
            settle_zero,
            || false,
            || {
                let next = script
                    .borrow_mut()
                    .pop_front()
                    .expect("read past the script");
                async move { next }
            },
        )
        .await
    }

    fn failed() -> Result<u16> {
        Err(anyhow::anyhow!("no answer"))
    }

    #[tokio::test]
    async fn zero_on_every_read_settles_at_zero_over_the_cable() {
        assert_eq!(probe(vec![Ok(0), Ok(0), Ok(0)], true).await, Some(0));
    }

    #[tokio::test]
    async fn zero_over_bluetooth_stays_unknown() {
        assert_eq!(probe(vec![Ok(0), Ok(0), Ok(0)], false).await, None);
    }

    #[tokio::test]
    async fn a_zero_followed_by_no_answer_stays_unknown() {
        assert_eq!(probe(vec![Ok(0), failed(), failed()], true).await, None);
    }

    #[tokio::test]
    async fn a_zero_is_settled_only_by_the_final_read_answering_zero() {
        assert_eq!(probe(vec![failed(), Ok(0), Ok(0)], true).await, Some(0));
        assert_eq!(probe(vec![Ok(0), Ok(0), failed()], true).await, None);
    }

    #[tokio::test]
    async fn a_positive_release_is_reported_at_once() {
        assert_eq!(probe(vec![Ok(0), Ok(3)], false).await, Some(3));
    }

    #[tokio::test]
    async fn a_closed_handler_stops_the_probe() {
        let reads = RefCell::new(0);
        let out = probe_net_core(
            &[0, 0],
            true,
            || true,
            || {
                *reads.borrow_mut() += 1;
                async { Ok(0) }
            },
        )
        .await;
        assert_eq!(out, None);
        assert_eq!(*reads.borrow(), 0);
    }
}
