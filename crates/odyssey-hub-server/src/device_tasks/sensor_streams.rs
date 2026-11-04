use futures::stream::{BoxStream, SelectAll, StreamExt as _};
use odyssey_usb::device::VmDevice;
use odyssey_usb::packets::vm::{
    AccelReport, BatteryReport, CombinedMarkersReport, ImpactReport, PocMarkersReport, VendorData,
};
use tracing::{debug, info, warn};

pub(super) enum DeviceStreamEvent {
    CombinedMarkers(CombinedMarkersReport),
    PocMarkers(PocMarkersReport),
    Accel(AccelReport),
    Impact(ImpactReport),
    Battery(BatteryReport),
    Vendor(u8, VendorData),
}

/// Start all sensor streams for a given events device.
/// Returns a SelectAll of boxed streams.
pub(super) async fn start_sensor_streams(
    events_device: &VmDevice,
    uuid: [u8; 6],
    protocol_version: [u16; 3],
) -> SelectAll<BoxStream<'static, DeviceStreamEvent>> {
    info!(
        "start_sensor_streams for {:02x?}: protocol_version={}.{}.{}",
        uuid, protocol_version[0], protocol_version[1], protocol_version[2]
    );
    let mut sensor_streams: SelectAll<BoxStream<'static, DeviceStreamEvent>> = SelectAll::new();
    match events_device.clone().stream_combined_markers().await {
        Ok(stream) => {
            info!("Combined markers stream started for {:02x?}", uuid);
            sensor_streams.push(stream.map(DeviceStreamEvent::CombinedMarkers).boxed());
        }
        Err(e) => warn!(
            "Failed to start combined markers stream for {:02x?}: {}",
            uuid, e
        ),
    }
    match events_device.clone().stream_poc_markers().await {
        Ok(stream) => {
            info!("PoC markers stream started for {:02x?}", uuid);
            sensor_streams.push(stream.map(DeviceStreamEvent::PocMarkers).boxed());
        }
        Err(e) => warn!(
            "Failed to start PoC markers stream for {:02x?}: {}",
            uuid, e
        ),
    }
    match events_device.clone().stream_accel().await {
        Ok(stream) => {
            info!("Accelerometer stream started for {:02x?}", uuid);
            sensor_streams.push(stream.map(DeviceStreamEvent::Accel).boxed());
        }
        Err(e) => warn!(
            "Failed to start accelerometer stream for {:02x?}: {}",
            uuid, e
        ),
    }
    match events_device.clone().stream_impact().await {
        Ok(stream) => {
            info!("Impact stream started for {:02x?}", uuid);
            sensor_streams.push(stream.map(DeviceStreamEvent::Impact).boxed());
        }
        Err(e) => warn!("Failed to start impact stream for {:02x?}: {}", uuid, e),
    }
    if protocol_version >= [0, 1, 1] {
        match events_device.clone().stream_battery().await {
            Ok(stream) => {
                info!("Battery stream started for {:02x?}", uuid);
                sensor_streams.push(stream.map(DeviceStreamEvent::Battery).boxed());
            }
            Err(e) => warn!("Failed to start battery stream for {:02x?}: {}", uuid, e),
        }
    } else {
        debug!(
            "Skipping battery stream for {:02x?}: protocol version {}.{}.{} < 0.1.1",
            uuid, protocol_version[0], protocol_version[1], protocol_version[2]
        );
    }
    sensor_streams
}
