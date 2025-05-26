use ahrs::Ahrs;
use arc_swap::ArcSwap;
use arrayvec::ArrayVec;
use ats_cv::foveated::FoveatedAimpointState;
use ats_cv::{calculate_rotational_offset, to_normalized_image_coordinates, ScreenCalibration};
use ats_usb::device::UsbDevice;
use ats_usb::packets::vm::{CombinedMarkersReport, PocMarkersReport};
use core::panic;
use nalgebra::{Isometry3, Point2, Rotation3, Translation3, UnitVector3, Vector3};
use odyssey_hub_common::device::{CdcDevice, Device, UdpDevice};
use opencv_ros_camera::RosOpenCvIntrinsics;
use serialport::SerialPortType;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use sysinfo::{Networks, System};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::sync::{mpsc::Sender, Mutex};
use tokio::time::sleep;
use tokio_stream::{Stream, StreamExt};
#[allow(unused_imports)]
use tracing::field::debug;

#[derive(Debug, Clone)]
pub enum DeviceTaskMessage {
    ResetZero,
    SaveZero,
    Zero(Translation3<f32>, Point2<f32>),
}

#[derive(Debug, Clone)]
pub enum Message {
    Connect(
        odyssey_hub_common::device::Device,
        ats_usb::device::UsbDevice,
        Sender<DeviceTaskMessage>,
    ),
    Disconnect(odyssey_hub_common::device::Device),
    Event(odyssey_hub_common::events::Event),
}

pub async fn device_tasks(
    message_channel: Sender<Message>,
    screen_calibrations: Arc<
        ArcSwap<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
    device_offsets: Arc<Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,
) -> anyhow::Result<()> {
    tokio::select! {
        _ = device_udp_manager(message_channel.clone(), screen_calibrations.clone(), device_offsets.clone()) => {},
        _ = device_cdc_manager(message_channel.clone(), screen_calibrations.clone(), device_offsets.clone()) => {},
    }
    Ok(())
}

pub async fn device_udp_manager(
    outer_tx: mpsc::Sender<Message>,
    screen_calibrations: Arc<
        arc_swap::ArcSwap<
            arrayvec::ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
    device_offsets: Arc<Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,
) {
    fn broadcast_address(ip: std::net::IpAddr, prefix: u8) -> Option<std::net::Ipv4Addr> {
        match ip {
            std::net::IpAddr::V4(ipv4) => {
                // Calculate the mask for IPv4 by shifting left
                let mask = !((1 << (32 - prefix)) - 1);
                let network = u32::from(ipv4) & mask;
                let broadcast = network | !mask;
                Some(Ipv4Addr::from(broadcast))
            }
            std::net::IpAddr::V6(_ipv6) => None,
        }
    }

    // internal channel for events from stream tasks
    let (ev_tx, mut ev_rx) = mpsc::channel::<Message>(32);

    // Map from port-string → task handle
    let mut handles: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    let mut devices: HashMap<String, UdpDevice> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();

    // prepare broadcast socket once
    let mut sys = System::new_all();
    sys.refresh_all();
    let networks = Networks::new_with_refreshed_list();
    let broadcast_addrs: Vec<Ipv4Addr> = networks
        .iter()
        .filter_map(|(_, data)| {
            data.ip_networks().iter().find_map(|ipn| {
                if let Some(broadcast) = broadcast_address(ipn.addr, ipn.prefix) {
                    if broadcast == Ipv4Addr::new(127, 255, 255, 255) {
                        Some(Ipv4Addr::new(127, 31, 33, 7))
                    } else {
                        Some(broadcast)
                    }
                } else {
                    None
                }
            })
        })
        .collect();

    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
    let addr = std::net::SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    sock.set_nonblocking(true).unwrap();
    sock.set_ttl(5).unwrap();
    sock.set_broadcast(true).unwrap();
    sock.bind(&socket2::SockAddr::from(addr)).unwrap();

    let socket = UdpSocket::from_std(sock.into()).unwrap();
    let mut buf = [0u8; 1472];

    loop {
        // 1) Ping phase
        let mut responders = HashSet::new();
        for _ in 0..5 {
            for &bcast in &broadcast_addrs {
                let _ = socket.send_to(&[255, 3], (bcast, 23456)).await;
            }
            let timeout = sleep(Duration::from_secs(1));
            tokio::pin!(timeout);

            loop {
                tokio::select! {
                    _ = &mut timeout => break,
                    Ok((len, addr)) = socket.recv_from(&mut buf) => {
                        if len >= 2 && buf[0] == 255 && buf[1] != 1 {
                            let id = buf[1];
                            let key = addr.to_string();
                            responders.insert(key.clone());

                            if !seen.contains(&key) {
                                seen.insert(key.clone());

                                let dev = Arc::new(Mutex::new(UdpDevice { id, addr, uuid: [0;6] }));
                                let ev_tx = ev_tx.clone();
                                let sc = screen_calibrations.clone();
                                let dev_o = device_offsets.clone();

                                let handle = tokio::spawn({
                                    let dev = dev.clone();
                                    let outer_tx = outer_tx.clone();
                                    async move {
                                        let _ = device_udp_stream_task(
                                            dev.clone(),
                                            ev_tx.clone(),
                                            outer_tx.clone(),
                                            sc.clone(),
                                            dev_o.clone(),
                                        )
                                        .await;

                                        let final_dev = dev.lock().await.clone();
                                        let _ = ev_tx
                                            .send(Message::Disconnect(Device::Udp(final_dev)))
                                            .await;
                                    }
                                });

                                handles.insert(key.clone(), handle);
                            }
                        }
                    }
                }
            }
        }

        let gone: Vec<String> = handles
            .keys()
            .filter(|k| !responders.contains(*k))
            .cloned()
            .collect();
        for key in gone {
            if let Some(h) = handles.remove(&key) {
                h.abort();

                if let Some(dev) = devices.remove(&key) {
                    let _ = outer_tx.send(Message::Disconnect(Device::Udp(dev))).await;
                }
                seen.remove(&key);
            }
        }

        while let Ok(msg) = ev_rx.try_recv() {
            let _ = outer_tx.send(msg.clone()).await;

            if let Message::Connect(Device::Udp(dev), _, _) = msg {
                devices.insert(dev.addr.to_string(), dev);
            }
        }

        sleep(Duration::from_secs(2)).await;
    }
}

async fn device_cdc_manager(
    outer_tx: mpsc::Sender<Message>,
    screen_calibrations: Arc<
        arc_swap::ArcSwap<
            arrayvec::ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
    device_offsets: Arc<Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,
) {
    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::channel::<Message>(32);

    let mut handles: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    let mut devices: HashMap<String, CdcDevice> = HashMap::new();
    let mut seen_paths: HashSet<String> = HashSet::new();

    loop {
        let ports = serialport::available_ports().unwrap_or_default();
        let mut matching_paths = HashSet::new();
        for p in &ports {
            if let SerialPortType::UsbPort(pi) = &p.port_type {
                if pi.vid == 0x1915
                    && matches!(pi.pid, 0x520f | 0x5210 | 0x5211)
                    && pi.interface.unwrap_or(0) == 0
                {
                    matching_paths.insert(p.port_name.clone());
                }
            }
        }

        let unplugged: Vec<String> = handles
            .keys()
            .filter(|path| !matching_paths.contains(*path))
            .cloned()
            .collect();
        for path in unplugged {
            if let Some(h) = handles.remove(&path) {
                h.abort();
            }
            if let Some(dev) = devices.remove(&path) {
                let _ = outer_tx.send(Message::Disconnect(Device::Cdc(dev))).await;
            }
            seen_paths.remove(&path);
        }

        for p in &ports {
            if let SerialPortType::UsbPort(pi) = &p.port_type {
                if pi.vid == 0x1915
                    && matches!(pi.pid, 0x520f | 0x5210 | 0x5211)
                    && pi.interface.unwrap_or(0) == 0
                {
                    let path = p.port_name.clone();
                    if seen_paths.insert(path.clone()) {
                        let device = Arc::new(Mutex::new(CdcDevice {
                            path: path.clone(),
                            uuid: [0; 6],
                        }));
                        let ev_tx = ev_tx.clone();
                        let sc = screen_calibrations.clone();
                        let dev_o = device_offsets.clone();
                        let pid_is_5210 = pi.pid == 0x5210;

                        let handle = tokio::spawn({
                            let device = device.clone();
                            let outer_tx = outer_tx.clone();
                            async move {
                                let _ = device_cdc_stream_task(
                                    device.clone(),
                                    pid_is_5210,
                                    ev_tx.clone(),
                                    outer_tx,
                                    sc,
                                    dev_o,
                                )
                                .await;
                                let final_dev = device.lock().await.clone();
                                let _ = ev_tx
                                    .send(Message::Disconnect(Device::Cdc(final_dev)))
                                    .await;
                            }
                        });

                        handles.insert(path.clone(), handle);
                    }
                }
            }
        }

        while let Ok(msg) = ev_rx.try_recv() {
            let _ = outer_tx.send(msg.clone()).await;

            if let Message::Connect(Device::Cdc(dev), _, _) = msg {
                devices.insert(dev.path.clone(), dev);
            }
        }

        sleep(Duration::from_secs(2)).await;
    }
}

pub struct Marker {
    pub normalized: Point2<f32>,
}

impl Marker {
    pub fn ats_cv_marker(&self) -> ats_cv::foveated::Marker {
        ats_cv::foveated::Marker {
            position: self.normalized,
        }
    }
}

async fn common_tasks(
    d: UsbDevice,
    device: Device,
    message_channel: Sender<Message>,
    mut config: ats_usb::packets::vm::GeneralConfig,
    screen_calibrations: Arc<
        ArcSwap<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
    device_offsets: Arc<Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,
    mut rx: tokio::sync::mpsc::Receiver<DeviceTaskMessage>,
) {
    println!("starting common_tasks for {device:?}");

    let uuid = match device {
        Device::Udp(ref device) => device.uuid,
        Device::Cdc(ref device) => device.uuid,
        Device::Hid(_) => unimplemented!(),
    };

    let mut init_fv_zero_offset = device_offsets.lock().await.get(&uuid).cloned();

    let fv_state = Arc::new(tokio::sync::Mutex::new(FoveatedAimpointState::new()));
    let timeout = Duration::from_secs(2);
    let restart_timeout = Duration::from_secs(1);

    let mut prev_timestamp: Option<u32> = None;
    let mut wfnf_realign = true;
    let orientation = Arc::new(tokio::sync::Mutex::<Rotation3<f32>>::new(
        nalgebra::Rotation3::identity(),
    ));
    let madgwick = Arc::new(tokio::sync::Mutex::new(ahrs::Madgwick::new(
        1. / config.accel_config.accel_odr as f32,
        0.1,
    )));
    let fv_zero_offset = ArcSwap::from(Arc::new(init_fv_zero_offset));

    type MarkerStream = Pin<Box<dyn Stream<Item = CombinedMarkersReport> + Send>>;

    fn to_stream<C, P>(c: C, p: P) -> MarkerStream
    where
        C: Stream<Item = CombinedMarkersReport> + Send + 'static,
        P: Stream<Item = PocMarkersReport> + Send + 'static,
    {
        Box::pin(c.merge(p.map(|x| x.into())))
    }

    let combined_markers_stream = d.stream_combined_markers().await.unwrap();
    let poc_markers_stream = d.stream_poc_markers().await.unwrap();
    let mut markers_stream = to_stream(combined_markers_stream, poc_markers_stream);

    let mut accel_stream = d.stream_accel().await.unwrap();
    let mut impact_stream = d.stream_impact().await.unwrap();
    let mut no_response_count = 0;
    let is_localhost = match device {
        Device::Udp(UdpDevice { addr, .. }) => match addr.ip() {
            IpAddr::V4(ip) => ip.is_loopback(),
            _ => false,
        },
        _ => false,
    };

    loop {
        tokio::select! {
            _ = tokio::time::sleep(
                if no_response_count > 0 {
                    restart_timeout
                } else {
                    timeout
                }
            ), if !is_localhost => {
                if no_response_count >= 5 {
                    tracing::info!(device=debug(&device), "no response, exiting");
                    break;
                }
                // nothing received after timeout, try restarting streams
                tracing::debug!(device=debug(&device), "common streams timed out, restarting streams");
                drop(markers_stream);
                drop(accel_stream);
                drop(impact_stream);
                tokio::time::sleep(Duration::from_millis(100)).await;
                let combined_markers_stream = d.stream_combined_markers().await.unwrap();
                let poc_markers_stream = d.stream_poc_markers().await.unwrap();
                markers_stream = to_stream(combined_markers_stream, poc_markers_stream);
                tokio::time::sleep(Duration::from_millis(50)).await;
                accel_stream = d.stream_accel().await.unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
                impact_stream = d.stream_impact().await.unwrap();
                no_response_count += 1;
                continue;
            }
            item = markers_stream.next() => {
                let Some(report) = item else {
                    // this shouldn't ever happen
                    break;
                };
                let CombinedMarkersReport { nf_points, wf_points } = report;

                let pose;
                let aimpoint_and_d;

                // Helper closure to process points
                let process_points = |points, camera_model, stereo_iso| {
                    let point_tuples = create_point_tuples(points);
                    let points_raw: Vec<_> = point_tuples.iter().map(|&(_, p)| p).collect();
                    let points_transformed = transform_points(&points_raw, camera_model);
                    let intrinsics = ats_common::ros_opencv_intrinsics_type_convert(camera_model);
                    let normalized_points: ArrayVec<_, 16> = points_transformed
                        .iter()
                        .map(|&p| to_normalized_image_coordinates(p, &intrinsics, stereo_iso))
                        .collect();
                    let markers: Vec<_> = point_tuples
                        .iter()
                        .zip(&normalized_points)
                        .map(|(&(_, _), &normalized)| Marker {
                            normalized,
                        })
                        .collect();
                    (point_tuples, points_transformed, normalized_points, markers)
                };

                // Process nf_points and wf_points
                let (_, _, nf_normalized, nf_markers2) =
                    process_points(&nf_points, &config.camera_model_nf, None);
                let (_, _, wf_normalized, wf_markers2) =
                    process_points(
                        &wf_points,
                        &config.camera_model_wf,
                        Some(&config.stereo_iso.cast()),
                    );

                let gravity_vec = orientation.lock().await.inverse_transform_vector(&Vector3::z_axis());
                let gravity_vec = UnitVector3::new_unchecked(gravity_vec.xzy());

                // Re-alignment logic
                if wfnf_realign {
                    let screen_calibrations = screen_calibrations.load();
                    if let Some((wf_match_ix, _, _)) = ats_cv::foveated::identify_markers(
                        &wf_normalized,
                        gravity_vec.cast(),
                        &screen_calibrations,
                    ) {
                        let wf_match = wf_match_ix.map(|i| wf_normalized[i].coords);
                        let (nf_match_ix, _) = ats_cv::foveated::match3(&nf_normalized, &wf_match);
                        if nf_match_ix.iter().all(Option::is_some) {
                            let nf_ordered =
                                nf_match_ix.map(|i| nf_normalized[i.unwrap()].coords.push(1.0));
                            let wf_ordered = wf_match_ix.map(|i| wf_normalized[i].coords.push(1.0));
                            let q = calculate_rotational_offset(&wf_ordered, &nf_ordered);
                            config.stereo_iso.rotation *= q.cast();
                            wfnf_realign = false;
                        }
                    }
                }

                let screen_id: u32;

                {
                    let mut fv_state = fv_state.lock().await;

                    let screen_calibrations = screen_calibrations.load();

                    let nf_markers_cv = nf_markers2
                        .iter()
                        .map(|m| m.ats_cv_marker())
                        .collect::<ArrayVec<_, 16>>();
                    let wf_markers_cv = wf_markers2
                        .iter()
                        .map(|m| m.ats_cv_marker())
                        .collect::<ArrayVec<_, 16>>();
                    fv_state.observe_markers(
                        &nf_markers_cv,
                        &wf_markers_cv,
                        gravity_vec.cast(),
                        &screen_calibrations,
                    );

                    (pose, aimpoint_and_d) = ats_cv::helpers::raycast_update(&screen_calibrations, &mut fv_state, **fv_zero_offset.load());
                    screen_id = fv_state.screen_id as u32;
                }

                if let Some((aimpoint, d)) = aimpoint_and_d { if let Some(pose) = pose {
                    let aimpoint_matrix = nalgebra::Matrix::<f32, nalgebra::Const<2>, nalgebra::Const<1>, nalgebra::ArrayStorage<f32, 2, 1>>::from_column_slice(&[aimpoint.x.into(), aimpoint.y.into()]);
                    let device = device.clone();
                    let kind = odyssey_hub_common::events::DeviceEventKind::TrackingEvent(odyssey_hub_common::events::TrackingEvent {
                        timestamp: prev_timestamp.unwrap_or(0),
                        aimpoint: aimpoint_matrix.cast(),
                        pose: Some(odyssey_hub_common::events::Pose {
                            rotation: pose.0.cast(),
                            translation: pose.1.cast(),
                        }),
                        distance: d,
                        screen_id,
                    });
                    match device {
                        Device::Udp(device) => {
                            let _ = message_channel.send(Message::Event(odyssey_hub_common::events::Event::DeviceEvent(
                                odyssey_hub_common::events::DeviceEvent {
                                    device: Device::Udp(device.clone()),
                                    kind,
                                }
                            ))).await;
                        },
                        Device::Cdc(device) => {
                            let _ = message_channel.send(Message::Event(odyssey_hub_common::events::Event::DeviceEvent(
                                odyssey_hub_common::events::DeviceEvent {
                                    device: Device::Cdc(device.clone()),
                                    kind,
                                }
                            ))).await;
                        },
                        Device::Hid(_) => {},
                    }
                }}
            }
            item = accel_stream.next() => {
                let Some(accel) = item else {
                    // this shouldn't ever happen
                    break;
                };

                // correct accel and gyro bias and scale
                let accel = ats_usb::packets::vm::AccelReport {
                    accel: accel.corrected_accel(&config.accel_config),
                    gyro: accel.corrected_gyro(&config.gyro_config),
                    timestamp: accel.timestamp,
                };

                if let Some(_prev_timestamp) = prev_timestamp {
                    if accel.timestamp < _prev_timestamp {
                        prev_timestamp = None;
                        continue;
                    }
                }

                let _orientation;

                {
                    let mut madgwick = madgwick.lock().await;

                    if let Some(prev_timestamp) = prev_timestamp {
                        let elapsed = accel.timestamp as u64 - prev_timestamp as u64;
                        // println!("elapsed: {}", elapsed);
                        fv_state.lock().await.predict(-accel.accel.xzy(), -accel.gyro.xzy(), Duration::from_micros(elapsed));

                        let sample_period = madgwick.sample_period_mut();
                        *sample_period = elapsed as f32/1_000_000.;
                    } else {
                        fv_state.lock().await.predict(-accel.accel.xzy(), -accel.gyro.xzy(), Duration::from_secs_f32(1./config.accel_config.accel_odr as f32));
                    }

                    let _ = madgwick.update_imu(&Vector3::from(accel.gyro), &Vector3::from(accel.accel));
                    _orientation = madgwick.quat.to_rotation_matrix();
                }

                prev_timestamp = Some(accel.timestamp);

                let euler_angles = _orientation.euler_angles();
                let euler_angles = Vector3::new(euler_angles.0, euler_angles.1, euler_angles.2);
                *orientation.lock().await = _orientation;
                {
                    let device = device.clone();
                    let kind = odyssey_hub_common::events::DeviceEventKind::AccelerometerEvent(odyssey_hub_common::events::AccelerometerEvent {
                        timestamp: prev_timestamp.unwrap_or(0),
                        accel: accel.accel.cast(),
                        gyro: accel.gyro.cast(),
                        euler_angles,
                    });
                    match device {
                        Device::Udp(device) => {
                            let _ = message_channel.send(Message::Event(odyssey_hub_common::events::Event::DeviceEvent(
                                odyssey_hub_common::events::DeviceEvent {
                                    device: Device::Udp(device.clone()),
                                    kind,
                                }
                            ))).await;
                        },
                        Device::Cdc(device) => {
                            let _ = message_channel.send(Message::Event(odyssey_hub_common::events::Event::DeviceEvent(
                                odyssey_hub_common::events::DeviceEvent {
                                    device: Device::Cdc(device.clone()),
                                    kind,
                                }
                            ))).await;
                        },
                        Device::Hid(_) => {},
                    }
                }
            }
            item = impact_stream.next() => {
                let Some(impact) = item else {
                    // this shouldn't ever happen
                    break;
                };
                let _ = message_channel.send(Message::Event(odyssey_hub_common::events::Event::DeviceEvent(
                    odyssey_hub_common::events::DeviceEvent {
                        device: device.clone(),
                        kind: odyssey_hub_common::events::DeviceEventKind::ImpactEvent(odyssey_hub_common::events::ImpactEvent {
                            timestamp: impact.timestamp,
                        }),
                    }
                ))).await;
            }
            item = rx.recv() => {
                let Some(message) = item else {
                    // this shouldn't ever happen
                    break;
                };
                match message {
                    DeviceTaskMessage::ResetZero => {
                        fv_zero_offset.store(Arc::new(init_fv_zero_offset));
                    },
                    DeviceTaskMessage::SaveZero => {
                        init_fv_zero_offset = *fv_zero_offset.load().clone();
                        if let Some(offset) = init_fv_zero_offset {
                            device_offsets.lock().await.insert(uuid, offset);
                        } else {
                            device_offsets.lock().await.remove(&uuid);
                        }
                        let device_offsets = device_offsets.lock().await.clone();
                        tokio::task::spawn_blocking(move || {
                            match odyssey_hub_common::config::save_device_offsets(&device_offsets) {
                                Ok(_) => {
                                    tracing::info!("Saved device offsets");
                                }
                                Err(e) => {
                                    tracing::error!("Failed to save device offsets: {}", e);
                                }
                            }
                        });
                    },
                    DeviceTaskMessage::Zero(t, point) => {
                        let quat = {
                            let screen_calibrations = screen_calibrations.load();
                            let fv_state = fv_state.lock().await;
                            ats_cv::helpers::calculate_zero_offset_quat(t, point, &screen_calibrations, &fv_state)
                        };
                        if let Some(quat) = quat {
                            fv_zero_offset.store(Arc::new(Some(Isometry3::from_parts(
                                t,
                                quat,
                            ))));
                            let _ = message_channel.send(Message::Event(odyssey_hub_common::events::Event::DeviceEvent(
                                odyssey_hub_common::events::DeviceEvent {
                                    device: device.clone(),
                                    kind: odyssey_hub_common::events::DeviceEventKind::ZeroResult(true),
                                }
                            ))).await;
                        } else {
                            let _ = message_channel.send(Message::Event(odyssey_hub_common::events::Event::DeviceEvent(
                                odyssey_hub_common::events::DeviceEvent {
                                    device: device.clone(),
                                    kind: odyssey_hub_common::events::DeviceEventKind::ZeroResult(false),
                                }
                            ))).await;
                        }
                    },
                }
            }
        }
        no_response_count = 0;
    }
    tracing::debug!("common_tasks for {device:?} exiting");
}

pub async fn device_udp_stream_task(
    device: Arc<Mutex<UdpDevice>>,
    slow_message_channel: Sender<Message>,
    message_channel: Sender<Message>,
    screen_calibrations: Arc<
        arc_swap::ArcSwap<
            arrayvec::ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
    device_offsets: Arc<Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,
) -> anyhow::Result<()> {
    let addr = { device.lock().await.addr.clone() };

    let d = match UsbDevice::connect_hub("0.0.0.0:0", &addr.to_string()).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Failed to connect to device {}: {}", addr, e);
            return Ok(());
        }
    };

    tracing::info!("Connected to device {}", addr);

    let timeout = tokio::time::Duration::from_millis(500);

    let config = match retry(|| d.read_config(), timeout, 3).await {
        Some(x) => x?,
        None => {
            return Err(anyhow::Error::msg("Failed to read config"));
        }
    };
    let props = match retry(|| d.read_props(), timeout, 3).await {
        Some(x) => x?,
        None => {
            return Err(anyhow::Error::msg("Failed to read props"));
        }
    };

    {
        let mut dev = device.lock().await;
        dev.uuid = props.uuid.clone();
    }

    let (tx, rx) = tokio::sync::mpsc::channel(5);

    let device = device.lock().await.clone();

    slow_message_channel
        .send(Message::Connect(Device::Udp(device.clone()), d.clone(), tx))
        .await?;

    common_tasks(
        d,
        Device::Udp(device.clone()),
        message_channel.clone(),
        config,
        screen_calibrations,
        device_offsets,
        rx,
    )
    .await;

    Ok(())
}

pub async fn device_cdc_stream_task(
    device: Arc<Mutex<CdcDevice>>,
    wait_dsr: bool,
    slow_message_channel: Sender<Message>,
    message_channel: Sender<Message>,
    screen_calibrations: Arc<
        ArcSwap<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
    device_offsets: Arc<Mutex<HashMap<[u8; 6], Isometry3<f32>>>>,
) -> anyhow::Result<()> {
    let path = device.lock().await.path.clone();

    let d = match UsbDevice::connect_serial(&path, wait_dsr).await {
        Ok(d) => d,
        Err(_) => return Ok(()),
    };

    let config = d.read_config().await?;
    let props = d.read_props().await?;

    {
        let mut dev = device.lock().await;
        dev.uuid = props.uuid;
    }

    let (tx, rx) = tokio::sync::mpsc::channel(5);

    let device = device.lock().await.clone();

    slow_message_channel
        .send(Message::Connect(Device::Cdc(device.clone()), d.clone(), tx))
        .await?;

    tokio::select! {
        _ = common_tasks(
            d.clone(),
            Device::Cdc(device.clone()),
            message_channel.clone(),
            config,
            screen_calibrations,
            device_offsets,
            rx,
        ) => {}
        _ = temp_boneless_hardcoded_vendor_stream_tasks(
            d.clone(),
            Device::Cdc(device.clone()),
            message_channel.clone(),
        ) => {}
    };

    println!("Exiting device_cdc_stream_task");

    Ok(())
}

// stream generic from 0x81 to 0x83 including 0x87 VendorEvents
async fn temp_boneless_hardcoded_vendor_stream_tasks(
    d: UsbDevice,
    device: Device,
    message_channel: Sender<Message>,
) {
    let mut vendor_streams: Vec<_> = (0x81..=0x83).collect();
    vendor_streams.push(0x87);
    vendor_streams.push(0x90);

    let vendor_streams: Vec<_> = vendor_streams
        .into_iter()
        .map(|i| {
            let d = d.clone();
            async move { d.stream(ats_usb::packets::vm::PacketType::Vendor(i)).await }
        })
        .collect();

    let vendor_tasks: Vec<_> = vendor_streams
        .into_iter()
        .map(|s| {
            let message_channel = message_channel.clone();
            let device = device.clone();
            tokio::spawn(async move {
                let mut stream = s.await.unwrap();
                while let Some(data) = stream.next().await {
                    let kind = odyssey_hub_common::events::DeviceEventKind::PacketEvent(
                        ats_usb::packets::vm::Packet {
                            id: 255,
                            data: data.clone(),
                        },
                    );
                    match device {
                        Device::Udp(ref device) => {
                            let _ = message_channel
                                .send(Message::Event(
                                    odyssey_hub_common::events::Event::DeviceEvent(
                                        odyssey_hub_common::events::DeviceEvent {
                                            device: Device::Udp(device.clone()),
                                            kind,
                                        },
                                    ),
                                ))
                                .await;
                        }
                        Device::Cdc(ref device) => {
                            let _ = message_channel
                                .send(Message::Event(
                                    odyssey_hub_common::events::Event::DeviceEvent(
                                        odyssey_hub_common::events::DeviceEvent {
                                            device: Device::Cdc(device.clone()),
                                            kind,
                                        },
                                    ),
                                ))
                                .await;
                        }
                        Device::Hid(_) => {}
                    }
                }
            })
        })
        .collect();

    match futures::future::select_all(vendor_tasks).await {
        (Ok(_), _, _) => {}
        (Err(e), _, _) => {
            eprintln!("Error in vendor stream task: {}", e);
        }
    }
}

fn create_point_tuples(points: &[Point2<u16>]) -> Vec<(u8, Point2<f32>)> {
    points
        .iter()
        .enumerate()
        .filter(|(_id, pos)| **pos != Point2::new(0, 0))
        .map(|(id, pos)| (id as u8, Point2::new(pos.x as f32, pos.y as f32)))
        .collect()
}

fn transform_points(
    points: &[Point2<f32>],
    camera_intrinsics: &RosOpenCvIntrinsics<f32>,
) -> Vec<Point2<f32>> {
    ats_cv::undistort_points(
        &ats_common::ros_opencv_intrinsics_type_convert(camera_intrinsics),
        &points,
    )
}

/// Retry an asynchronous operation up to `limit` times.
async fn retry<F, G>(mut op: F, timeout: tokio::time::Duration, limit: usize) -> Option<G::Output>
where
    F: FnMut() -> G,
    G: std::future::Future,
{
    for _ in 0..limit {
        match tokio::time::timeout(timeout, op()).await {
            Ok(r) => return Some(r),
            Err(_) => (),
        }
    }
    None
}
