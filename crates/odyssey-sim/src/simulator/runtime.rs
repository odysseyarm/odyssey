use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use odyssey_protocol::Packet;
use odyssey_protocol::control::device::DeviceMsg;
use odyssey_protocol::control::usb_mux::UsbMuxCtrlMsg;
use odyssey_protocol::mux::MuxMsg;
use tokio::sync::{Notify, mpsc, watch};

use super::model::Model;
use super::{ClockMode, Pending, Request, Snapshot, Uuid, mark_stopped};

struct Publication(watch::Sender<Snapshot>);

impl Drop for Publication {
    fn drop(&mut self) {
        self.0.send_modify(mark_stopped);
    }
}

/// The host's side of one endpoint. `controls` may close before `packets`.
struct Endpoint<P, C> {
    packets: mpsc::Receiver<P>,
    controls: Option<mpsc::Receiver<C>>,
}

enum Received<P, C> {
    Packet(P),
    Control(C),
    /// The host dropped the endpoint.
    Closed,
}

/// The next message from any endpoint, with the address it arrived at.
async fn receive<P, C>(endpoints: &mut BTreeMap<Uuid, Endpoint<P, C>>) -> (Uuid, Received<P, C>) {
    std::future::poll_fn(|context| {
        for (uuid, endpoint) in endpoints.iter_mut() {
            if let Poll::Ready(packet) = endpoint.packets.poll_recv(context) {
                return Poll::Ready((*uuid, packet.map_or(Received::Closed, Received::Packet)));
            }
            if let Some(controls) = &mut endpoint.controls {
                match controls.poll_recv(context) {
                    Poll::Ready(Some(message)) => {
                        return Poll::Ready((*uuid, Received::Control(message)));
                    }
                    Poll::Ready(None) => endpoint.controls = None,
                    Poll::Pending => {}
                }
            }
        }
        Poll::Pending
    })
    .await
}

pub(crate) async fn run(
    clock: ClockMode,
    mut model: Model,
    mut commands: mpsc::Receiver<Request>,
    snapshots: watch::Sender<Snapshot>,
    pending_connections: Arc<Mutex<Vec<Pending>>>,
    notify: Arc<Notify>,
) {
    let publication = Publication(snapshots);
    let mut units: BTreeMap<Uuid, Endpoint<Packet, DeviceMsg>> = BTreeMap::new();
    let mut dongles: BTreeMap<Uuid, Endpoint<MuxMsg, UsbMuxCtrlMsg>> = BTreeMap::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(10));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;
    // A late tick advances by the time that passed, so simulator time keeps
    // pace with the wall clock that clients schedule poses against.
    let mut last_tick = tokio::time::Instant::now();
    loop {
        tokio::select! {
            // A newly opened endpoint is installed before requests queued on it.
            biased;
            () = notify.notified() => {
                let opened = std::mem::take(&mut *pending_connections.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
                for connection in opened {
                    match connection {
                        Pending::Unit(uuid, connection) => {
                            if let Some(unit) = model.units.get_mut(&uuid) && unit.plugged {
                                unit.disconnect_usb();
                                unit.usb = Some(connection.replies);
                                unit.control = Some(connection.events);
                                units.insert(uuid, Endpoint { packets: connection.packets, controls: Some(connection.controls) });
                            }
                        }
                        Pending::Dongle(uuid, connection) => {
                            if let Some(dongle) = model.dongles.get_mut(&uuid) && dongle.plugged {
                                dongle.disconnect_mux();
                                dongle.mux = Some(connection.replies);
                                dongle.control = Some(connection.events);
                                dongles.insert(uuid, Endpoint { packets: connection.packets, controls: Some(connection.controls) });
                            }
                        }
                    }
                }
            },
            request = commands.recv() => {
                let Some(request) = request else { break; };
                let result = model.command(request.target, request.command);
                if let Err(error) = &result { model.errors.report(error.clone()); }
                publication.0.send_modify(|snapshot| model.publish(snapshot));
                let _ = request.response.send(result);
            },
            (uuid, received) = receive(&mut units) => {
                match received {
                    Received::Packet(packet) => model.unit_packet(uuid, packet),
                    Received::Control(message) => model.unit_control(uuid, message),
                    Received::Closed => {
                        if let Some(unit) = model.units.get_mut(&uuid) { unit.disconnect_usb(); }
                    }
                }
            },
            (uuid, received) = receive(&mut dongles) => {
                match received {
                    Received::Packet(message) => model.mux_message(uuid, message),
                    Received::Control(message) => model.dongle_control(uuid, message),
                    Received::Closed => {
                        if let Some(dongle) = model.dongles.get_mut(&uuid) { dongle.disconnect_mux(); }
                    }
                }
            },
            _ = ticker.tick(), if clock == ClockMode::Realtime => {
                let now = tokio::time::Instant::now();
                let elapsed_ms = now.duration_since(last_tick).as_millis() as u64;
                last_tick += Duration::from_millis(elapsed_ms);
                model.tick(model.time_ms.saturating_add(elapsed_ms));
            },
        }
        units.retain(|uuid, _| model.units.get(uuid).is_some_and(|unit| unit.usb.is_some()));
        dongles.retain(|uuid, _| {
            model
                .dongles
                .get(uuid)
                .is_some_and(|dongle| dongle.mux.is_some())
        });
        publication
            .0
            .send_modify(|snapshot| model.publish(snapshot));
    }
}
