//! Exercises the model through the production USB and mux client dispatchers.
#![expect(
    clippy::print_stdout,
    reason = "the smoke executable reports its observed result"
)]
use std::time::Duration;

use eros::Context as _;
use odyssey_protocol::control::device::TransportMode;
use odyssey_protocol::image::{IMAGE_LEASE_TIMEOUT_MS, ImageStatusCode, ImageStreamConfig};
use odyssey_protocol::{ConfigKind, GeneralConfig, PacketData, Port, PropKind, Props};
use odyssey_sim::{
    Command, DONGLE_UUID, ScenarioStep, SensorModel, Simulator, Target, Transport, UNIT_UUID,
    unit_uuid,
};
use odyssey_usb::device::{PairingEvent, VmDevice};
use tokio_stream::StreamExt as _;

#[tokio::main(flavor = "current_thread")]
async fn main() -> eros::Result<()> {
    let sim = Simulator::new();
    let handle = sim.handle();
    handle.execute(Command::UnitUsb { plugged: true }).await?;
    handle.execute(Command::DongleUsb { plugged: true }).await?;
    let usb = sim.connect_unit_usb(UNIT_UUID);
    let dongle = sim.connect_dongle(DONGLE_UUID);
    assert_eq!(usb.read_uuid().await?, UNIT_UUID);
    assert_eq!(usb.get_transport_mode().await?, TransportMode::Usb);
    assert_eq!(
        usb.set_transport_mode(TransportMode::Ble).await?,
        TransportMode::Usb
    );
    let version = usb.read_version().await?;
    assert_eq!(
        version,
        match usb.request(PacketData::ReadVersion()).await? {
            PacketData::ReadVersionResponse(version) => version,
            other => eros::bail!("unexpected version response {other:?}"),
        }
    );
    usb.write_config(GeneralConfig::ImpactThreshold(73)).await?;
    assert_eq!(
        usb.read_config(ConfigKind::ImpactThreshold).await?,
        GeneralConfig::ImpactThreshold(73)
    );
    usb.write_register(Port::Nf, 2, 3, 0xa5).await?;
    assert_eq!(usb.read_register(Port::Nf, 2, 3).await?, 0xa5);
    usb.read_all_config().await?;
    let mut errors = handle.errors();
    usb.write_vendor(0x90, &[1, 2]).await?;
    usb.read_prop(PropKind::Uuid).await?;
    assert!(matches!(
        errors.recv().await?,
        odyssey_sim::SimError::Unsupported(_)
    ));
    dongle.start_pairing(5000).await?;
    usb.start_pairing(5000).await?;
    assert_eq!(usb.wait_pairing_event().await?.uuid, DONGLE_UUID);
    assert!(
        matches!(dongle.wait_pairing_event().await?, PairingEvent::Result(device) if device.uuid == UNIT_UUID)
    );
    assert_eq!(dongle.list_bonds().await?[0].uuid, UNIT_UUID);
    assert_eq!(dongle.request_devices().await?.as_slice(), &[UNIT_UUID]);

    let mut accel = usb.stream_accel().await?;
    let report = tokio::time::timeout(Duration::from_secs(2), accel.next())
        .await?
        .context("USB accelerometer stream closed")?;
    assert_eq!(report.accel.z, 9.80665);
    let mut markers = usb.stream_poc_markers().await?;
    let before = tokio::time::timeout(Duration::from_secs(2), markers.next())
        .await?
        .context("marker stream closed")?;
    handle.execute(Command::Aim { x: 0.75, y: 0.5 }).await?;
    let after = loop {
        let report = tokio::time::timeout(Duration::from_secs(2), markers.next())
            .await?
            .context("marker stream closed")?;
        if report.points[0].x != before.points[0].x {
            break report;
        }
    };
    assert!(after.points[0].x < before.points[0].x);

    let image_config = ImageStreamConfig {
        camera: Port::Nf,
        fps: 15,
    };
    let mut images = usb.stream_images(image_config).await?;
    assert!(matches!(
        usb.stream_images(image_config).await,
        Err(odyssey_usb::Error::ImageStatus(ImageStatusCode::Busy))
    ));
    let first_image = tokio::time::timeout(Duration::from_secs(3), images.next())
        .await?
        .context("image stream closed")??;
    assert_eq!((first_image.width, first_image.height), (320, 240));
    assert_eq!(first_image.pixels.len(), 320 * 240);
    assert!(first_image.objects.is_empty());
    assert_eq!(first_image.objects_timestamp_us, 0);
    loop {
        tokio::select! {
            biased;
            report = markers.next() => { report.context("marker stream closed during capture")?; }
            () = std::future::ready(()) => break,
        }
    }
    handle.execute(Command::Aim { x: 0.6, y: 0.5 }).await?;
    let next_image = tokio::select! {
        biased;
        _ = markers.next() => panic!("tracking reports must pause during image capture"),
        frame = tokio::time::timeout(Duration::from_secs(3), images.next()) =>
            frame?.context("image stream closed")??,
    };
    assert!(next_image.objects.is_empty());
    assert_eq!(next_image.objects_timestamp_us, 0);
    let cadence_us = next_image
        .timestamp_us
        .wrapping_sub(first_image.timestamp_us);
    assert!(
        (66_000..200_000).contains(&cadence_us),
        "15 FPS request must exceed 5 FPS; simulated cadence was {cadence_us} µs"
    );
    // Sensor capture must not starve control or the independent IMU stream.
    assert_eq!(
        usb.read_config(ConfigKind::ImpactThreshold).await?,
        GeneralConfig::ImpactThreshold(73)
    );
    loop {
        let sample = tokio::time::timeout(Duration::from_secs(1), accel.next())
            .await?
            .context("IMU stream closed during capture")?;
        if sample.timestamp >= next_image.timestamp_us {
            assert_eq!(sample.accel.z, 9.80665);
            break;
        }
    }
    images.stop().await?;
    handle.execute(Command::Aim { x: 0.6, y: 0.5 }).await?;
    loop {
        let marker = tokio::time::timeout(Duration::from_secs(2), markers.next())
            .await?
            .context("marker stream did not resume after capture")?;
        if marker.points[0].x != after.points[0].x {
            assert!(marker.points[0].x > after.points[0].x);
            break;
        }
    }
    let images = usb
        .stream_images(ImageStreamConfig {
            camera: Port::Nf,
            fps: 5,
        })
        .await?;
    drop(images);
    handle.execute(Command::Aim { x: 0.75, y: 0.5 }).await?;
    loop {
        let marker = tokio::time::timeout(Duration::from_secs(2), markers.next())
            .await?
            .context("marker stream did not resume after capture drop")?;
        if marker.points[0].x == after.points[0].x {
            break;
        }
    }
    let mut replacement = usb
        .stream_images(ImageStreamConfig {
            camera: Port::Nf,
            fps: 5,
        })
        .await?;
    let replacement_frame = tokio::time::timeout(Duration::from_secs(3), replacement.next())
        .await?
        .context("replacement image stream closed")??;
    assert_eq!(
        (replacement_frame.width, replacement_frame.height),
        (320, 240)
    );
    replacement.stop().await?;
    let lease = Duration::from_millis(IMAGE_LEASE_TIMEOUT_MS);
    let modern_nf = ImageStreamConfig {
        camera: Port::Nf,
        fps: 5,
    };
    let mut renewed = usb.stream_images(modern_nf).await?;
    let renewing_since = tokio::time::Instant::now();
    while renewing_since.elapsed() < lease + Duration::from_millis(1500) {
        renewed.renew_lease()?;
        tokio::time::timeout(Duration::from_secs(3), renewed.next())
            .await?
            .context("renewed image stream closed")??;
    }
    renewed.stop().await?;
    let mut abandoned = usb.stream_images(modern_nf).await?;
    tokio::time::timeout(Duration::from_secs(3), abandoned.next())
        .await?
        .context("abandoned image stream closed")??;
    // Markers are paused now, so anything still buffered predates this session.
    while let Ok(Some(_)) = tokio::time::timeout(Duration::ZERO, markers.next()).await {}
    let expired = tokio::time::timeout(lease + Duration::from_secs(2), async {
        loop {
            match abandoned.next().await {
                Some(Ok(_)) => {}
                end => break end,
            }
        }
    })
    .await?;
    assert!(
        matches!(
            expired,
            Some(Err(odyssey_usb::Error::ImageStatus(
                ImageStatusCode::LeaseExpired
            )))
        ),
        "unrenewed capture must expire, got {expired:?}"
    );
    tokio::time::timeout(Duration::from_secs(2), markers.next())
        .await?
        .context("tracking did not resume after lease expiry")?;
    abandoned.stop().await?;
    let mut impact = usb.stream_impact().await?;
    // A request/reply fences stream registration before the cross-channel manual command.
    usb.read_prop(PropKind::Uuid).await?;
    handle.execute(Command::Shot).await?;
    let shot = tokio::time::timeout(Duration::from_secs(2), impact.next())
        .await?
        .context("impact stream closed")?;
    assert!(shot.timestamp > 0);
    drop(accel);
    drop(markers);
    drop(impact);
    usb.clear_all_streams().await?;

    handle.execute(Command::UnitUsb { plugged: false }).await?;
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.units[0].transport, Transport::Ble);
    assert_eq!(snapshot.units[0].dongle, Some(DONGLE_UUID));
    assert!(usb.read_uuid().await.is_err());
    let ble = VmDevice::open_via_mux(dongle.clone(), UNIT_UUID);
    assert!(matches!(
        ble.read_prop(PropKind::Uuid).await?,
        Props::Uuid(UNIT_UUID)
    ));
    assert_eq!(ble.read_register(Port::Nf, 2, 3).await?, 0xa5);
    assert!(matches!(
        ble.stream_images(image_config).await,
        Err(odyssey_usb::Error::ImageUsbRequired)
    ));
    let mut battery = ble.stream_battery().await?;
    ble.read_prop(PropKind::Uuid).await?;
    handle
        .execute(Command::Battery {
            percent: 42,
            charging: false,
        })
        .await?;
    loop {
        let report = tokio::time::timeout(Duration::from_secs(2), battery.next())
            .await?
            .context("BLE battery stream closed")?;
        if report.percent == 42 {
            break;
        }
    }
    drop(battery);
    ble.clear_all_streams().await?;
    handle
        .execute(Command::BleRange { in_range: false })
        .await?;
    assert!(dongle.request_devices().await?.is_empty());
    handle.execute(Command::BleRange { in_range: true }).await?;
    assert_eq!(dongle.request_devices().await?.as_slice(), &[UNIT_UUID]);
    handle.execute(Command::UnitUsb { plugged: true }).await?;
    let reconnected = sim.connect_unit_usb(UNIT_UUID);
    assert_eq!(reconnected.read_uuid().await?, UNIT_UUID);
    dongle.clear_bonds().await?;
    assert!(!handle.snapshot().units[0].bonded);
    assert!(dongle.list_bonds().await?.is_empty());
    assert!(dongle.request_devices().await?.is_empty());
    handle
        .execute(Command::DongleUsb { plugged: false })
        .await?;
    handle.execute(Command::DongleUsb { plugged: true }).await?;
    let new_dongle = sim.connect_dongle(DONGLE_UUID);
    assert!(new_dongle.request_devices().await?.is_empty());

    let second = unit_uuid(1);
    handle.execute(Command::AddUnit { uuid: second }).await?;
    handle
        .execute_on(Target::unit(second), Command::UnitUsb { plugged: true })
        .await?;
    let second_usb = sim.connect_unit_usb(second);
    assert_eq!(second_usb.read_uuid().await?, second);
    assert_eq!(reconnected.read_uuid().await?, UNIT_UUID);
    handle
        .execute_on(Target::unit(second), Command::Pair { timeout_ms: 1000 })
        .await?;
    assert_eq!(new_dongle.request_devices().await?.as_slice(), &[second]);
    let second_ble = VmDevice::open_via_mux(new_dongle.clone(), second);
    assert!(matches!(
        second_ble.read_prop(PropKind::Uuid).await?,
        Props::Uuid(uuid) if uuid == second
    ));
    handle.execute(Command::RemoveUnit { uuid: second }).await?;
    assert!(new_dongle.request_devices().await?.is_empty());
    assert!(second_usb.read_uuid().await.is_err());
    sim.shutdown().await?;
    assert!(!handle.snapshot().running);
    assert!(handle.execute(Command::Shot).await.is_err());
    let manual = Simulator::with_clock(odyssey_sim::ClockMode::Manual);
    let control = manual.handle();
    control.execute(Command::Pair { timeout_ms: 25 }).await?;
    control.execute(Command::Advance { millis: 25 }).await?;
    let expired = control.snapshot();
    assert!(!expired.units[0].pairing && !expired.dongles[0].pairing && !expired.units[0].bonded);
    let step = |after_ms, command| ScenarioStep {
        after_ms,
        target: Target::default(),
        command,
    };
    let scripted = manual
        .run_scenario(&[
            step(5, Command::Pair { timeout_ms: 100 }),
            step(5, Command::CancelPairing),
            step(5, Command::Aim { x: 0.25, y: 0.75 }),
        ])
        .await?;
    assert_eq!(scripted.time_ms, 40);
    assert_eq!(scripted.units[0].aim, Some([0.25, 0.75]));
    assert!(!scripted.units[0].pairing && !scripted.dongles[0].pairing);
    let failed = manual
        .run_scenario(&[
            step(
                0,
                Command::Battery {
                    percent: 101,
                    charging: false,
                },
            ),
            step(0, Command::Aim { x: 0.5, y: 0.5 }),
        ])
        .await;
    assert!(matches!(
        failed,
        Err(odyssey_sim::SimError::Scenario { index: 0, .. })
    ));
    assert_eq!(control.snapshot().units[0].aim, Some([0.25, 0.75]));
    manual.shutdown().await?;
    legacy_images().await?;
    println!(
        "simulator smoke passed: USB protocol/config/registers, image-only pixels above 5 FPS, tracking pause/resume, independent control/IMU, stop/drop/restart, legacy NF/WF reboot recovery, BLE image rejection, subscribed reports, pairing/bonds, USB→BLE failover, range, hotplug, shutdown"
    );
    Ok(())
}

async fn legacy_images() -> eros::Result<()> {
    let sim = Simulator::with_sensor_model(SensorModel::Paj7025);
    let handle = sim.handle();
    handle.execute(Command::UnitUsb { plugged: true }).await?;
    for camera in [Port::Nf, Port::Wf] {
        let usb = sim.connect_unit_usb(UNIT_UUID);
        let mut images = usb
            .stream_images(ImageStreamConfig { camera, fps: 5 })
            .await?;
        let frame = tokio::time::timeout(Duration::from_secs(2), images.next())
            .await?
            .context("legacy image stream closed")??;
        assert_eq!((frame.camera, frame.width, frame.height), (camera, 98, 98));
        assert!(
            frame.objects.is_empty(),
            "legacy image mode cannot sample objects"
        );
        assert_eq!(frame.objects_timestamp_us, 0);
        assert!(matches!(
            images.stop().await,
            Err(odyssey_usb::Error::ImageStatus(
                ImageStatusCode::RebootRequired
            ))
        ));
        assert!(matches!(
            usb.stream_images(ImageStreamConfig { camera, fps: 5 })
                .await,
            Err(odyssey_usb::Error::ImageStatus(
                ImageStatusCode::RebootRequired
            ))
        ));
        let generation = handle.snapshot().units[0].usb_generation;
        let mut snapshots = handle.subscribe();
        usb.reboot().await?;
        tokio::time::timeout(
            Duration::from_secs(2),
            snapshots.wait_for(|state| state.units[0].usb_generation != generation),
        )
        .await??;
    }
    let usb = sim.connect_unit_usb(UNIT_UUID);
    let mut markers = usb.stream_poc_markers().await?;
    let before = tokio::time::timeout(Duration::from_secs(2), markers.next())
        .await?
        .context("legacy tracking did not resume after reboot")?;
    handle.execute(Command::Aim { x: 0.6, y: 0.5 }).await?;
    let after = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let report = markers.next().await.context("legacy tracking ended")?;
            if report.points[0].x != before.points[0].x {
                break eros::Result::<_>::Ok(report);
            }
        }
    })
    .await??;
    assert!(after.points[0].x < before.points[0].x);
    sim.shutdown().await?;
    Ok(())
}
