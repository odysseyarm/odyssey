use std::future::pending;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use odyssey_protocol::Packet;
use odyssey_protocol::control::device::DeviceMsg;
use odyssey_protocol::control::usb_mux::UsbMuxCtrlMsg;
use odyssey_protocol::mux::MuxMsg;
use tokio::sync::{Notify, broadcast, mpsc, watch};

use super::model::Model;
use super::{ClockMode, Pending, Request, SensorModel, SimError, Snapshot};

struct Publication(watch::Sender<Snapshot>);

impl Drop for Publication {
    fn drop(&mut self) {
        self.0.send_modify(|snapshot| {
            snapshot.running = false;
            snapshot.ble_connected = false;
        });
    }
}

async fn receive<T>(receiver: &mut Option<mpsc::Receiver<T>>) -> Option<T> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => pending().await,
    }
}

pub(crate) async fn run(
    clock: ClockMode,
    sensor_model: SensorModel,
    mut commands: mpsc::Receiver<Request>,
    snapshots: watch::Sender<Snapshot>,
    errors: broadcast::Sender<SimError>,
    pending_connections: Arc<Mutex<Pending>>,
    notify: Arc<Notify>,
) {
    let publication = Publication(snapshots);
    let mut model = Model::new(errors, sensor_model);
    let mut usb_packets: Option<mpsc::Receiver<Packet>> = None;
    let mut usb_controls: Option<mpsc::Receiver<DeviceMsg>> = None;
    let mut mux_packets: Option<mpsc::Receiver<MuxMsg>> = None;
    let mut mux_controls: Option<mpsc::Receiver<UsbMuxCtrlMsg>> = None;
    let mut ticker = tokio::time::interval(Duration::from_millis(10));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;
    loop {
        tokio::select! {
            // A newly opened endpoint is installed before requests queued on it.
            biased;
            () = notify.notified() => {
                let Pending { usb, mux } = std::mem::take(&mut *pending_connections.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
                if let Some(connection) = usb && model.state.unit_usb {
                    model.disconnect_usb();
                    usb_packets = Some(connection.packets);
                    usb_controls = Some(connection.controls);
                    model.usb = Some(connection.replies);
                    model.unit_control = Some(connection.events);
                }
                if let Some(connection) = mux && model.state.dongle_usb {
                    mux_packets = Some(connection.packets);
                    mux_controls = Some(connection.controls);
                    model.mux = Some(connection.replies);
                    model.mux_control = Some(connection.events);
                }
            },
            request = commands.recv() => {
                let Some(request) = request else { break; };
                let result = model.command(request.command);
                if let Err(error) = &result { model.error(error.clone()); }
                publication.0.send_replace(model.state.clone());
                let _ = request.response.send(result.map(|()| model.state.clone()));
            },
            packet = receive(&mut usb_packets) => {
                match packet {
                    Some(packet) => model.packet(packet, false),
                    None => { usb_packets = None; model.disconnect_usb(); },
                }
            },
            control = receive(&mut usb_controls) => {
                match control {
                    Some(message) => model.unit_control(message),
                    None => usb_controls = None,
                }
            },
            packet = receive(&mut mux_packets) => {
                match packet {
                    Some(packet) => model.mux_packet(packet),
                    None => { mux_packets = None; model.disconnect_mux(); },
                }
            },
            control = receive(&mut mux_controls) => {
                match control {
                    Some(message) => model.dongle_control(message),
                    None => mux_controls = None,
                }
            },
            _ = ticker.tick(), if clock == ClockMode::Realtime => {
                model.tick(model.state.time_ms.saturating_add(10));
            },
        }
        if model.usb.is_none() {
            usb_packets = None;
            usb_controls = None;
        }
        if model.mux.is_none() {
            mux_packets = None;
            mux_controls = None;
        }
        publication.0.send_replace(model.state.clone());
    }
}
