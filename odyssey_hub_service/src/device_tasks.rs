use ahrs::Ahrs;
use arrayvec::ArrayVec;
use ats_cv::foveated::{match3, FoveatedAimpointState};
use ats_cv::{calculate_rotational_offset, to_normalized_image_coordinates, ScreenCalibration};
use ats_usb::device::UsbDevice;
use ats_usb::packet::CombinedMarkersReport;
use core::panic;
use nalgebra::{Matrix3, Point2, Rotation3, Translation3, UnitVector3, Vector3};
use odyssey_hub_common::device::{CdcDevice, Device, UdpDevice};
use opencv_ros_camera::RosOpenCvIntrinsics;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::Sender;
use tokio_stream::StreamExt;
#[allow(unused_imports)]
use tracing::field::debug;

#[derive(Debug, Clone)]
pub enum Message {
    Connect(
        odyssey_hub_common::device::Device,
        ats_usb::device::UsbDevice,
    ),
    Disconnect(odyssey_hub_common::device::Device),
    Event(odyssey_hub_common::events::Event),
}

pub async fn device_tasks(
    message_channel: Sender<Message>,
    screen_calibrations: Arc<
        tokio::sync::Mutex<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
) -> anyhow::Result<()> {
    tokio::select! {
        _ = device_udp_ping_task(message_channel.clone(), screen_calibrations.clone()) => {},
        _ = device_hid_ping_task(message_channel.clone(), screen_calibrations.clone()) => {},
        _ = device_cdc_ping_task(message_channel.clone(), screen_calibrations.clone()) => {},
    }
    Ok(())
}

async fn device_udp_ping_task(
    message_channel: Sender<Message>,
    screen_calibrations: Arc<
        tokio::sync::Mutex<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
) -> std::convert::Infallible {
    use sysinfo::{Components, Disks, Networks, System};

    fn broadcast_address(ip: std::net::IpAddr, prefix: u8) -> Option<std::net::IpAddr> {
        match ip {
            std::net::IpAddr::V4(ipv4) => {
                // Calculate the mask for IPv4 by shifting left
                let mask = !((1 << (32 - prefix)) - 1);
                let network = u32::from(ipv4) & mask;
                let broadcast = network | !mask;
                Some(std::net::IpAddr::V4(Ipv4Addr::from(broadcast)))
            }
            std::net::IpAddr::V6(ipv6) => {
                None
            }
        }
    }

    let mut sys = System::new_all();
    sys.refresh_all();

    let networks = Networks::new_with_refreshed_list();

    let broadcast_addrs: Vec<_> = networks
        .iter()
        .filter_map(|(_, data)| {
            for ip_network in data.ip_networks() {
                if let Some(broadcast) = broadcast_address(ip_network.addr, ip_network.prefix) {
                    return Some(broadcast);
                }
            }
            None
        })
        .collect();

    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )
    .unwrap();
    let addr = std::net::SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    socket.set_nonblocking(true).unwrap();
    socket.set_ttl(5).unwrap();
    socket.set_broadcast(true).unwrap();
    socket.bind(&socket2::SockAddr::from(addr)).unwrap();
    let socket = UdpSocket::from_std(socket.into()).unwrap();

    let (sender, mut receiver) = tokio::sync::mpsc::channel(12);

    let stream_task_handles = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    tokio::select! {
        _ = async {
            loop {
                let mut buf = [0; 1472];
                let mut devices_that_responded = Vec::new();

                // 5 attempts
                for _ in 0..5 {
                    // ping without add
                    for ip in &broadcast_addrs {
                        match ip {
                            std::net::IpAddr::V4(broadcast_address) => {
                                let broadcast_address = SocketAddrV4::new(*broadcast_address, 23456);
                                socket.send_to(&[255, 3], broadcast_address).await.unwrap();
                            },
                            std::net::IpAddr::V6(_) => {
                                // unsupported
                            },
                        }
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {},
                        _ = async {
                            let stream_task_handles = stream_task_handles.clone();
                            let message_channel = message_channel.clone();
                            let screen_calibrations = screen_calibrations.clone();
                            loop {
                                let (_len, addr) = socket.recv_from(&mut buf).await.unwrap();
                                if buf[0] == 255 || buf[1] != 1 /* Ping */ { continue; }
                                let device = odyssey_hub_common::device::UdpDevice { id: buf[1], addr, uuid: [0; 6]};
                                let mut stream_task_handles = stream_task_handles.lock().await;
                                let message_channel = message_channel.clone();
                                let sender = sender.clone();
                                let screen_calibrations = screen_calibrations.clone();

                                if !devices_that_responded.contains(&device) {
                                    devices_that_responded.push(device.clone());
                                }

                                let i = stream_task_handles.iter().position(|(a, _)| *a == device);
                                if let None = i {
                                    stream_task_handles.push((
                                        device.clone(),
                                        tokio::spawn({
                                            async move {
                                                match device_udp_stream_task(device.clone(), message_channel.clone(), screen_calibrations.clone()).await {
                                                    Ok(_) => {
                                                        println!("UDP device stream task finished");
                                                    },
                                                    Err(e) => {
                                                        eprintln!("Error in device stream task: {}", e);
                                                    }
                                                }
                                                let _ = sender.send(Message::Disconnect(odyssey_hub_common::device::Device::Udp(device.clone()))).await;
                                            }
                                        })
                                    ));
                                }
                            }
                        } => {},
                    }
                }

                for (device, _) in stream_task_handles.lock().await.iter() {
                    if !devices_that_responded.contains(device) {
                        let _ = sender.send(Message::Disconnect(odyssey_hub_common::device::Device::Udp(device.clone()))).await;
                    }
                }
            }
        } => panic!("UDP ping loop is supposed to be infallible"),
        _ = async {
            while let Some(message) = receiver.recv().await {
                match message {
                    Message::Disconnect(d) => {
                        if let Device::Udp(d) = d {
                            tracing::info!("Disconnecting {:?}", d);
                            let mut stream_task_handles = stream_task_handles.lock().await;
                            let i = stream_task_handles.iter().position(|(a, _)| *a == d);
                            if let Some(i) = i {
                                stream_task_handles[i].1.abort();
                                stream_task_handles.remove(i);
                                message_channel.send(Message::Disconnect(odyssey_hub_common::device::Device::Udp(d.clone()))).await.unwrap();
                            }
                        }
                    },
                    _ => {},
                }
            }
        } => panic!("UDP ping receiver is supposed to be infallible"),
    }
}

async fn device_hid_ping_task(
    message_channel: Sender<Message>,
    _screen_calibrations: Arc<
        tokio::sync::Mutex<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
) -> std::convert::Infallible {
    let api = hidapi::HidApi::new().unwrap();

    let mut old_list = vec![];
    loop {
        let mut new_list = vec![];
        for device in api.device_list() {
            if device.vendor_id() == 0x1915 && device.product_id() == 0x48AB {
                if !old_list.contains(&odyssey_hub_common::device::Device::Hid(
                    odyssey_hub_common::device::HidDevice {
                        path: device.path().to_str().unwrap().to_string(),
                        uuid: [0; 6],
                    },
                )) {
                    // todo
                    // let _ = message_channel
                    //     .send(Message::Connect(odyssey_hub_common::device::Device::Hid(
                    //         odyssey_hub_common::device::HidDevice {
                    //             path: device.path().to_str().unwrap().to_string(),
                    //             uuid: [0; 6],
                    //         },
                    //     )))
                    //     .await;
                }
                new_list.push(odyssey_hub_common::device::Device::Hid(
                    odyssey_hub_common::device::HidDevice {
                        path: device.path().to_str().unwrap().to_string(),
                        uuid: [0; 6],
                    },
                ));
            }
        }
        // dbg!(&new_list);
        for v in &old_list {
            if !new_list.contains(v) {
                let _ = message_channel.send(Message::Disconnect(v.clone())).await;
            }
        }
        old_list = new_list;
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    }
}

async fn device_cdc_ping_task(
    message_channel: Sender<Message>,
    screen_calibrations: Arc<
        tokio::sync::Mutex<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
) -> std::convert::Infallible {
    let stream_task_handles = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let old_list = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    loop {
        let new_list = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stream_task_handles = stream_task_handles.clone();

        let ports = serialport::available_ports();
        let ports: Vec<_> = match ports {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to list serial ports {}", &e.to_string());
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                continue;
            }
        }
        .into_iter()
        .filter_map(|port| {
            match &port.port_type {
                serialport::SerialPortType::UsbPort(port_info) => {
                    if port_info.vid != 0x1915
                        || !(port_info.pid == 0x520f || port_info.pid == 0x5210)
                    {
                        return None;
                    }
                    if let Some(i) = port_info.interface {
                        // interface 0: cdc acm module
                        // interface 1: cdc acm module functional subordinate interface
                        // interface 2: cdc acm dfu
                        // interface 3: cdc acm dfu subordinate interface
                        if i == 0 {
                            Some((port.clone(), port_info.clone()))
                        } else {
                            None
                        }
                    } else {
                        Some((port.clone(), port_info.clone()))
                    }
                }
                _ => None,
            }
        })
        .collect();
        for (port, port_info) in ports {
            let device = odyssey_hub_common::device::CdcDevice {
                path: port.port_name.clone(),
                uuid: [0; 6],
            };
            let old_list_arc_clone = old_list.clone();
            let old_list = old_list.lock().await;
            if !old_list.contains(&device) {
                stream_task_handles.lock().await.push((
                    device.clone(),
                    tokio::spawn({
                        let stream_task_handles = stream_task_handles.clone();
                        let old_list_arc_clone = old_list_arc_clone.clone();
                        let message_channel = message_channel.clone();
                        let device = device.clone();
                        let screen_calibrations = screen_calibrations.clone();
                        async move {
                            {
                                let message_channel = message_channel.clone();
                                match device_cdc_stream_task(
                                    device.clone(),
                                    port_info.pid == 0x5210,
                                    message_channel,
                                    screen_calibrations.clone(),
                                )
                                .await
                                {
                                    Ok(_) => {}
                                    Err(e) => {
                                        eprintln!("Error in device stream task: {}", e);
                                    }
                                }
                            }
                            stream_task_handles
                                .lock()
                                .await
                                .retain(|&(ref x, _)| x != &device);
                            let _ = message_channel
                                .send(Message::Disconnect(Device::Cdc(device.clone())))
                                .await;
                            old_list_arc_clone.lock().await.retain(|x| x != &device);
                        }
                    }),
                ));
            }
            new_list.lock().await.push(device);
        }

        // dbg!(&new_list);
        let new_list = new_list.lock().await;
        for v in old_list.lock().await.iter() {
            if !new_list.contains(v) {
                let _ = message_channel
                    .send(Message::Disconnect(Device::Cdc(v.clone())))
                    .await;
                let mut stream_task_handles = stream_task_handles.lock().await;
                if let Some((_, handle)) = stream_task_handles.iter().find(|x| x.0 == *v) {
                    handle.abort();
                    stream_task_handles.retain(|x| x.0 != *v);
                }
            }
        }
        *old_list.lock().await = new_list.to_vec();

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    }
}

fn get_raycast_aimpoint(
    fv_state: &ats_cv::foveated::FoveatedAimpointState,
    screen_calibration: &ScreenCalibration<f32>,
) -> (Rotation3<f32>, Translation3<f32>, Option<Point2<f32>>) {
    let orientation = fv_state.filter.orientation.cast();
    let position = fv_state.filter.position.cast();

    let rot = orientation.to_rotation_matrix();
    let trans = Translation3::from(position);

    let isometry = nalgebra::Isometry::<f32, Rotation3<f32>, 3>::from_parts(trans, rot);

    let fv_aimpoint = ats_cv::calculate_aimpoint(&isometry, screen_calibration);

    let flip_yz = Matrix3::new(1., 0., 0., 0., -1., 0., 0., 0., -1.);

    let rot = Rotation3::from_matrix_unchecked(flip_yz * rot * flip_yz);
    let trans = Translation3::from(flip_yz * trans.vector);

    (rot, trans, fv_aimpoint)
}

pub struct Marker {
    pub mot_id: u8,
    pub pattern_id: Option<u8>,
    pub normalized: Point2<f32>,
}

impl Marker {
    pub fn ats_cv_marker(&self) -> ats_cv::foveated::Marker {
        ats_cv::foveated::Marker {
            position: self.normalized,
        }
    }
}

fn raycast_update(
    screen_calibrations: &ArrayVec<
        (u8, ScreenCalibration<f32>),
        { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
    >,
    fv_state: &mut FoveatedAimpointState,
) -> (Option<(Matrix3<f32>, Vector3<f32>)>, Option<Point2<f32>>) {
    if !fv_state.init() {
        return (None, None);
    }
    let screen_id = fv_state.screen_id;
    if screen_id <= ats_cv::foveated::MAX_SCREEN_ID {
        if let Some(screen_calibration) =
            screen_calibrations
                .iter()
                .find_map(|(id, cal)| if *id == screen_id { Some(cal) } else { None })
        {
            let (rotmat, transmat, _fv_aimpoint) =
                get_raycast_aimpoint(&fv_state, screen_calibration);

            if let Some(_fv_aimpoint) = _fv_aimpoint {
                if _fv_aimpoint.x > 1.02 || _fv_aimpoint.x < -0.02 {
                    fv_state.reset();
                } else {
                    return (
                        Some((rotmat.matrix().cast(), transmat.vector.cast())),
                        Some(_fv_aimpoint),
                    );
                }
            } else {
                return (Some((rotmat.matrix().cast(), transmat.vector.cast())), None);
            }
        }
    }
    (None, None)
}

async fn common_tasks(
    d: UsbDevice,
    device: Device,
    message_channel: Sender<Message>,
    mut config: ats_usb::packet::GeneralConfig,
    screen_calibrations: Arc<
        tokio::sync::Mutex<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
) {
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

    let mut combined_markers_stream = d.stream_combined_markers().await.unwrap();
    let mut accel_stream = d.stream_accel().await.unwrap();
    let mut impact_stream = d.stream_impact().await.unwrap();
    let mut no_response_count = 0;

    loop {
        tokio::select! {
            _ = tokio::time::sleep(
                if no_response_count > 0 {
                    restart_timeout
                } else {
                    timeout
                }
            ) => {
                if no_response_count >= 5 {
                    tracing::info!(device=debug(&device), "no response, exiting");
                    break;
                }
                // nothing received after timeout, try restarting streams
                tracing::debug!(device=debug(&device), "common streams timed out, restarting streams");
                drop(combined_markers_stream);
                drop(accel_stream);
                drop(impact_stream);
                tokio::time::sleep(Duration::from_millis(100)).await;
                combined_markers_stream = d.stream_combined_markers().await.unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
                accel_stream = d.stream_accel().await.unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
                impact_stream = d.stream_impact().await.unwrap();
                no_response_count += 1;
                continue;
            }
            item = combined_markers_stream.next() => {
                let Some(combined_markers) = item else {
                    // this shouldn't ever happen
                    break;
                };
                let CombinedMarkersReport { nf_points, wf_points } = combined_markers;

                let pose;
                let aimpoint;

                let nf_point_tuples = filter_and_create_point_tuples(&nf_points);
                let wf_point_tuples = filter_and_create_point_tuples(&wf_points);

                let nf_points_slice = nf_point_tuples.iter().map(|(_, p)| *p).collect::<Vec<_>>();
                let wf_points_slice = wf_point_tuples.iter().map(|(_, p)| *p).collect::<Vec<_>>();

                let nf_points_transformed = transform_points(&nf_points_slice, &config.camera_model_nf);
                let wf_points_transformed = transform_points(&wf_points_slice, &config.camera_model_wf);

                let nf_point_tuples = nf_point_tuples.iter().enumerate().map(|(i, (id, _))| (*id, nf_points_transformed[i])).collect::<Vec<_>>();
                let wf_point_tuples = wf_point_tuples.iter().enumerate().map(|(i, (id, _))| (*id, wf_points_transformed[i])).collect::<Vec<_>>();

                let wf_normalized: ArrayVec<_, 16> = wf_points_transformed.iter().map(|&p| {
                    to_normalized_image_coordinates(
                        p,
                        &ats_cv::ros_opencv_intrinsics_type_convert(&config.camera_model_wf),
                        Some(&config.stereo_iso.cast()),
                    )
                }).collect();
                let nf_normalized: ArrayVec<_, 16> = nf_points_transformed.iter().map(|&p| {
                    to_normalized_image_coordinates(
                        p,
                        &ats_cv::ros_opencv_intrinsics_type_convert(&config.camera_model_nf),
                        None,
                    )
                }).collect();
                let nf_markers2: Vec<_> = std::iter::zip(&nf_normalized, &nf_point_tuples)
                    .map(|(&normalized, &(mot_id, _))| Marker {
                        mot_id,
                        pattern_id: None,
                        normalized,
                    })
                    .collect();
                let wf_markers2: Vec<_> = std::iter::zip(&wf_normalized, &wf_point_tuples)
                    .map(|(&normalized, &(mot_id, _))| Marker {
                        mot_id,
                        pattern_id: None,
                        normalized,
                    })
                    .collect();

                let gravity_vec = orientation.lock().await.inverse_transform_vector(&Vector3::z_axis());
                let gravity_vec = UnitVector3::new_unchecked(gravity_vec.xzy());
                if wfnf_realign {
                    // Try to match widefield using brute force p3p, and then
                    // using that to match nearfield

                    let screen_calibrations = screen_calibrations.lock().await;

                    if let Some((wf_match_ix, _, _)) = ats_cv::foveated::identify_markers(&wf_normalized, gravity_vec.cast(), &screen_calibrations) {
                        let wf_match = wf_match_ix.map(|i| wf_normalized[i].coords);
                        let (nf_match_ix, _) = match3(&nf_normalized, &wf_match);
                        if nf_match_ix.iter().all(Option::is_some) {
                            let nf_ordered = nf_match_ix.map(|i| nf_normalized[i.unwrap()].coords.push(1.0));
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

                    let screen_calibrations = screen_calibrations.lock().await;

                    let nf = &nf_markers2.iter().map(|m| m.ats_cv_marker()).collect::<ArrayVec<_, 16>>();
                    let wf = &wf_markers2.iter().map(|m| m.ats_cv_marker()).collect::<ArrayVec<_, 16>>();
                    fv_state.observe_markers(nf, wf, gravity_vec.cast(), &screen_calibrations);

                    let (_pose, _aimpoint) = raycast_update(&screen_calibrations, &mut fv_state);

                    pose = _pose;
                    aimpoint = _aimpoint;

                    screen_id = fv_state.screen_id as u32;
                }

                if let Some(aimpoint) = aimpoint { if let Some(pose) = pose {
                    let aimpoint_matrix = nalgebra::Matrix::<f32, nalgebra::Const<2>, nalgebra::Const<1>, nalgebra::ArrayStorage<f32, 2, 1>>::from_column_slice(&[aimpoint.x.into(), aimpoint.y.into()]);
                    let device = device.clone();
                    let kind = odyssey_hub_common::events::DeviceEventKind::TrackingEvent(odyssey_hub_common::events::TrackingEvent {
                        timestamp: prev_timestamp.unwrap_or(0),
                        aimpoint: aimpoint_matrix.cast(),
                        pose: Some(odyssey_hub_common::events::Pose {
                            rotation: pose.0.cast(),
                            translation: pose.1.cast(),
                        }),
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
                let accel = ats_usb::packet::AccelReport {
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

                    let _ = madgwick.update_imu(&Vector3::from(accel.gyro), &Vector3::from(accel.accel));
                    _orientation = madgwick.quat.to_rotation_matrix();

                    if let Some(prev_timestamp) = prev_timestamp {
                        let elapsed = accel.timestamp as u64 - prev_timestamp as u64;
                        // println!("elapsed: {}", elapsed);
                        fv_state.lock().await.predict(-accel.accel.xzy(), -accel.gyro.xzy(), Duration::from_micros(elapsed));

                        let sample_period = madgwick.sample_period_mut();
                        *sample_period = elapsed as f32/1_000_000.;
                    } else {
                        fv_state.lock().await.predict(-accel.accel.xzy(), -accel.gyro.xzy(), Duration::from_secs_f32(1./config.accel_config.accel_odr as f32));
                    }
                }

                prev_timestamp = Some(accel.timestamp);

                let euler_angles = _orientation.euler_angles();
                let euler_angles = Vector3::new(euler_angles.0 as f64, euler_angles.1 as f64, euler_angles.2 as f64);
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
        }
        no_response_count = 0;
    }
}

async fn device_udp_stream_task(
    device: UdpDevice,
    message_channel: Sender<Message>,
    screen_calibrations: Arc<
        tokio::sync::Mutex<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
) -> anyhow::Result<()> {
    let d = match UsbDevice::connect_hub("0.0.0.0:0", device.addr.to_string().as_str()).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to connect to device {}: {}", device.addr, e);
            return Ok(());
        }
    };

    tracing::info!("Connected to device {}", device.addr);

    let timeout = tokio::time::Duration::from_millis(500);
    let config = match retry(|| d.read_config(), timeout, 3).await {
        Some(x) => x?,
        None => {
            return Err(anyhow::Error::msg("Failed to read config"));
        }
    };
    let params = match retry(|| d.read_props(), timeout, 3).await {
        Some(x) => x?,
        None => {
            return Err(anyhow::Error::msg("Failed to read props"));
        }
    };

    let mut device = device.clone();
    device.uuid = params.uuid.clone();

    let _ = message_channel
        .send(Message::Connect(
            odyssey_hub_common::device::Device::Udp(device.clone()),
            d.clone(),
        ))
        .await;

    common_tasks(
        d,
        odyssey_hub_common::device::Device::Udp(device),
        message_channel,
        config,
        screen_calibrations.clone(),
    )
    .await;

    Ok(())
}

async fn device_cdc_stream_task(
    device: CdcDevice,
    wait_dsr: bool,
    message_channel: Sender<Message>,
    screen_calibrations: Arc<
        tokio::sync::Mutex<
            ArrayVec<
                (u8, ScreenCalibration<f32>),
                { (ats_cv::foveated::MAX_SCREEN_ID + 1) as usize },
            >,
        >,
    >,
) -> anyhow::Result<()> {
    let d = match UsbDevice::connect_serial(&device.path, wait_dsr).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to connect to device {}: {}", device.path, e);
            return Ok(());
        }
    };

    tracing::info!("Connected to device {}", device.path);

    let config = d.read_config().await?;
    let props = d.read_props().await?;

    let mut device = device.clone();
    device.uuid = props.uuid.clone();

    let _ = message_channel
        .send(Message::Connect(
            odyssey_hub_common::device::Device::Cdc(device.clone()),
            d.clone(),
        ))
        .await;

    tokio::select! {
        _ = common_tasks(
            d.clone(),
            odyssey_hub_common::device::Device::Cdc(device.clone()),
            message_channel.clone(),
            config,
            screen_calibrations.clone(),
        ) => {}
        _ = temp_boneless_hardcoded_vendor_stream_tasks(
            d.clone(),
            odyssey_hub_common::device::Device::Cdc(device.clone()),
            message_channel.clone(),
        ) => {}
    };

    Ok(())
}

// stream generic from 0x81 to 0x83 VendorEvents
async fn temp_boneless_hardcoded_vendor_stream_tasks(
    d: UsbDevice,
    device: Device,
    message_channel: Sender<Message>,
) {
    let vendor_streams: Vec<_> = (0x81..=0x83)
        .map(|i| {
            let d = d.clone();
            async move { d.stream(ats_usb::packet::PacketType::Vendor(i)).await }
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
                        ats_usb::packet::Packet {
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

fn filter_and_create_point_tuples(points: &[Point2<u16>]) -> Vec<(u8, Point2<f32>)> {
    points
        .iter()
        .enumerate()
        .filter_map(|(id, pos)| {
            // screen id of 7 means there is no marker
            if (100..3996).contains(&pos.x) && (100..3996).contains(&pos.y) {
                Some((id as u8, Point2::new(pos.x as f32, pos.y as f32)))
            } else {
                None
            }
        })
        .collect()
}

fn transform_points(
    points: &[Point2<f32>],
    camera_intrinsics: &RosOpenCvIntrinsics<f32>,
) -> Vec<Point2<f32>> {
    let scaled_points = points
        .iter()
        .map(|p| Point2::new(p.x / 4095. * 98., p.y / 4095. * 98.))
        .collect::<Vec<_>>();
    let undistorted_points = ats_cv::undistort_points(
        &ats_cv::ros_opencv_intrinsics_type_convert(camera_intrinsics),
        &scaled_points,
    );
    undistorted_points
        .iter()
        .map(|p| Point2::new(p.x / 98. * 4095., p.y / 98. * 4095.))
        .collect()
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
