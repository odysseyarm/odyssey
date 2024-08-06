use core::panic;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use ats_cv::foveated::FoveatedAimpointState;
use ats_cv::kalman::Pva2d;
use ats_usb::device::UsbDevice;
use ats_usb::packet::CombinedMarkersReport;
use nalgebra::{Matrix3, Point2, UnitVector3, Vector3};
use odyssey_hub_common::device::{CdcDevice, Device, UdpDevice};
use opencv_ros_camera::RosOpenCvIntrinsics;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::Sender;
use tokio_stream::StreamExt;
use tracing::field::debug;

#[derive(Debug, Clone)]
pub enum Message {
    Connect(odyssey_hub_common::device::Device),
    Disconnect(odyssey_hub_common::device::Device),
    Event(odyssey_hub_common::events::Event),
}

pub async fn device_tasks(message_channel: Sender<Message>) -> anyhow::Result<()> {
    tokio::select! {
        _ = device_udp_ping_task(message_channel.clone()) => {},
        _ = device_hid_ping_task(message_channel.clone()) => {},
        _ = device_cdc_ping_task(message_channel.clone()) => {},
    }
    Ok(())
}

async fn device_udp_ping_task(message_channel: Sender<Message>) -> std::convert::Infallible {
    let broadcast_addr = SocketAddrV4::new(Ipv4Addr::BROADCAST, 23456);

    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP)).unwrap();
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
                    socket.send_to(&[255, 3], broadcast_addr).await.unwrap();
                    tokio::select! {
                        _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {},
                        _ = async {
                            let stream_task_handles = stream_task_handles.clone();
                            let message_channel = message_channel.clone();
                            loop {
                                let (_len, addr) = socket.recv_from(&mut buf).await.unwrap();
                                if buf[0] == 255 || buf[1] != 1 /* Ping */ { continue; }
                                let device = odyssey_hub_common::device::UdpDevice { id: buf[1], addr, uuid: [0; 6]};
                                let mut stream_task_handles = stream_task_handles.lock().await;
                                let message_channel = message_channel.clone();
                                let sender = sender.clone();

                                if !devices_that_responded.contains(&device) {
                                    devices_that_responded.push(device.clone());
                                }

                                let i = stream_task_handles.iter().position(|(a, _)| *a == device);
                                if let None = i {
                                    stream_task_handles.push((
                                        device.clone(),
                                        tokio::spawn({
                                            async move {
                                                match device_udp_stream_task(device.clone(), message_channel.clone()).await {
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

async fn device_hid_ping_task(message_channel: Sender<Message>) -> std::convert::Infallible {
    let api = hidapi::HidApi::new().unwrap();

    let mut old_list = vec![];
    loop {
        let mut new_list = vec![];
        for device in api.device_list() {
            if device.vendor_id() == 0x1915 && device.product_id() == 0x48AB {
                if !old_list.contains(&odyssey_hub_common::device::Device::Hid(odyssey_hub_common::device::HidDevice { path: device.path().to_str().unwrap().to_string(), uuid: [0; 6]})) {
                    let _ = message_channel.send(Message::Connect(odyssey_hub_common::device::Device::Hid(odyssey_hub_common::device::HidDevice { path: device.path().to_str().unwrap().to_string(), uuid: [0; 6] }))).await;
                }
                new_list.push(odyssey_hub_common::device::Device::Hid(odyssey_hub_common::device::HidDevice { path: device.path().to_str().unwrap().to_string(), uuid: [0; 6]}));
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

async fn device_cdc_ping_task(message_channel: Sender<Message>) -> std::convert::Infallible {
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
        }.into_iter().filter_map(|port| {
            match &port.port_type {
                serialport::SerialPortType::UsbPort(port_info) => {
                    if port_info.vid != 0x1915 || !(port_info.pid == 0x520f || port_info.pid == 0x5210) {
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
                },
                _ => None,
            }
        }).collect();
        for (port, port_info) in ports {
            let device = odyssey_hub_common::device::CdcDevice { path: port.port_name.clone(), uuid: [0; 6] };
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
                        async move {
                            {
                                let message_channel = message_channel.clone();
                                match device_cdc_stream_task(device.clone(), port_info.pid == 0x5210, message_channel).await {
                                    Ok(_) => {},
                                    Err(e) => {
                                        eprintln!("Error in device stream task: {}", e);
                                    }
                                }
                            }
                            stream_task_handles.lock().await.retain(|&(ref x, _)| x != &device);
                            let _ = message_channel.send(Message::Disconnect(Device::Cdc(device.clone()))).await;
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
                let _ = message_channel.send(Message::Disconnect(Device::Cdc(v.clone()))).await;
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

fn get_raycast_aimpoint(fv_state: &ats_cv::foveated::FoveatedAimpointState) -> (Matrix3<f32>, nalgebra::Point3<f32>, Option<Point2<f32>>) {
    let orientation = fv_state.filter.orientation;
    let position = fv_state.filter.position;

    let flip_yz = Matrix3::new(
        1., 0., 0.,
        0., -1., 0.,
        0., 0., -1.,
    );

    let rotmat = flip_yz * orientation.to_rotation_matrix() * flip_yz;
    let transmat = flip_yz * position;

    let screen_3dpoints = ats_cv::calculate_screen_3dpoints(108., 16./9.);

    let fv_aimpoint = ats_cv::calculate_aimpoint_from_pose_and_screen_3dpoints(
        &rotmat,
        &transmat.coords,
        &screen_3dpoints,
    );

    (rotmat, transmat, fv_aimpoint)
}

async fn common_tasks(
    d: UsbDevice,
    device: Device,
    message_channel: Sender<Message>,
    orientation: Arc<tokio::sync::Mutex<nalgebra::Rotation3<f64>>>,
    config: ats_usb::packet::GeneralConfig,
) {
    let fv_state = Arc::new(tokio::sync::Mutex::new(FoveatedAimpointState::new()));
    let fv_aimpoint_pva2d = Arc::new(tokio::sync::Mutex::new(Pva2d::new(0.02, 1.0)));
    let timeout = Duration::from_secs(2);
    let restart_timeout = Duration::from_secs(1);

    let mut combined_markers_stream = d.stream_combined_markers().await.unwrap();
    let mut euler_stream = d.stream_euler_angles().await.unwrap();
    let mut impact_stream = d.stream_impact().await.unwrap();
    let mut no_response_count = 0;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(
                if no_response_count > 0 {
                    restart_timeout
                } else {
                    timeout
            }) => {
                if no_response_count >= 5 {
                    tracing::info!(device=debug(&device), "no response, exiting");
                    break;
                }
                // nothing received after timeout, try restarting streams
                tracing::debug!(device=debug(&device), "common streams timed out, restarting streams");
                drop(combined_markers_stream);
                drop(euler_stream);
                drop(impact_stream);
                tokio::time::sleep(Duration::from_millis(100)).await;
                combined_markers_stream = d.stream_combined_markers().await.unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
                euler_stream = d.stream_euler_angles().await.unwrap();
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
                let CombinedMarkersReport { timestamp, nf_points, wf_points, nf_radii, wf_radii } = combined_markers;

                let aim_point;
                let rotation_mat;
                let translation_mat;

                let filtered_nf_point_tuples = filter_and_create_point_id_tuples(&nf_points, &nf_radii);
                let filtered_wf_point_tuples = filter_and_create_point_id_tuples(&wf_points, &wf_radii);

                let filtered_nf_points_slice = filtered_nf_point_tuples.iter().map(|(_, p)| *p).collect::<Vec<_>>();
                let filtered_wf_points_slice = filtered_wf_point_tuples.iter().map(|(_, p)| *p).collect::<Vec<_>>();

                let nf_points_transformed = transform_points(&filtered_nf_points_slice, &config.camera_model_nf);
                let wf_points_transformed = transform_points(&filtered_wf_points_slice, &config.camera_model_wf);

                let wf_to_nf = ats_cv::wf_to_nf_points(
                    &wf_points_transformed,
                    &ats_cv::ros_opencv_intrinsics_type_convert(&config.camera_model_nf),
                    &ats_cv::ros_opencv_intrinsics_type_convert(&config.camera_model_wf),
                    config.stereo_iso.cast(),
                );

                let wf_normalized: Vec<_> = wf_to_nf.iter().map(|&p| {
                    let fx = config.camera_model_nf.p.m11 as f64;
                    let fy = config.camera_model_nf.p.m22 as f64;
                    let cx = config.camera_model_nf.p.m13 as f64;
                    let cy = config.camera_model_nf.p.m23 as f64;
                    Point2::new((p.x/4095.*98. - cx) / fx, (p.y/4095.*98. - cy) / fy)
                }).collect();
                let nf_normalized: Vec<_> = nf_points_transformed.iter().map(|&p| {
                    let fx = config.camera_model_nf.p.m11 as f64;
                    let fy = config.camera_model_nf.p.m22 as f64;
                    let cx = config.camera_model_nf.p.m13 as f64;
                    let cy = config.camera_model_nf.p.m23 as f64;
                    Point2::new((p.x/4095.*98. - cx) / fx, (p.y/4095.*98. - cy) / fy)
                }).collect();

                fv_aimpoint_pva2d.lock().await.step();

                let gravity_vec = orientation.lock().await.inverse_transform_vector(&Vector3::z_axis());
                let gravity_vec = UnitVector3::new_unchecked(gravity_vec.xzy());

                {
                    let mut fv_state = fv_state.lock().await;
                    fv_state.observe_markers(&nf_normalized, &wf_normalized, gravity_vec);

                    let (rotmat, transmat, fv_aimpoint) = get_raycast_aimpoint(&fv_state);

                    rotation_mat = rotmat.cast();
                    translation_mat = transmat.coords.cast();
                    aim_point = fv_aimpoint;
                }

                if let Some(aim_point) = aim_point {
                    let aim_point_matrix = nalgebra::Matrix::<f64, nalgebra::Const<2>, nalgebra::Const<1>, nalgebra::ArrayStorage<f64, 2, 1>>::from_column_slice(&[aim_point.x.into(), aim_point.y.into()]);
                    let device = device.clone();
                    let kind = odyssey_hub_common::events::DeviceEventKind::TrackingEvent(odyssey_hub_common::events::TrackingEvent {
                        timestamp,
                        aimpoint: aim_point_matrix,
                        pose: Some(odyssey_hub_common::events::Pose {
                            rotation: rotation_mat,
                            translation: translation_mat,
                        }),
                        screen_id: 0,
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
            item = euler_stream.next() => {
                let Some(_orientation) = item else {
                    // this shouldn't ever happen
                    break;
                };
                *orientation.lock().await = _orientation.rotation;
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

async fn device_udp_stream_task(device: UdpDevice, message_channel: Sender<Message>) -> anyhow::Result<()> {
    let d = match UsbDevice::connect_hub("0.0.0.0:0", device.addr.to_string().as_str()).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to connect to device {}: {}", device.addr, e);
            return Ok(());
        },
    };

    tracing::info!("Connected to device {}", device.addr);

    let timeout = tokio::time::Duration::from_millis(500);
    let config = match retry(|| d.read_config(), timeout, 3).await {
        Some(x) => x?,
        None => { return Err(anyhow::Error::msg("Failed to read config")); }
    };

    let mut device = device.clone();
    device.uuid = config.uuid.clone();

    let _ = message_channel.send(Message::Connect(odyssey_hub_common::device::Device::Udp(device.clone()))).await;

    let orientation = Arc::new(tokio::sync::Mutex::new(nalgebra::Rotation3::identity()));

    common_tasks(d, odyssey_hub_common::device::Device::Udp(device), message_channel, orientation, config).await;
    Ok(())
}

async fn device_cdc_stream_task(device: CdcDevice, wait_dsr: bool, message_channel: Sender<Message>) -> anyhow::Result<()> {
    let d = match UsbDevice::connect_serial(&device.path, wait_dsr).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to connect to device {}: {}", device.path, e);
            return Ok(());
        },
    };

    tracing::info!("Connected to device {}", device.path);

    let config = d.read_config().await?;

    let mut device = device.clone();
    device.uuid = config.uuid.clone();

    let _ = message_channel.send(Message::Connect(odyssey_hub_common::device::Device::Cdc(device.clone()))).await;

    let orientation = Arc::new(tokio::sync::Mutex::new(nalgebra::Rotation3::identity()));

    common_tasks(d, odyssey_hub_common::device::Device::Cdc(device), message_channel, orientation, config).await;
    Ok(())
}

fn filter_and_create_point_id_tuples(points: &[Point2<u16>], radii: &[u8]) -> Vec<(usize, Point2<f64>)> {
    points.iter().zip(radii.iter())
        .enumerate()
        .filter_map(|(id, (pos, &r))| if r > 0 { Some((id, Point2::new(pos.x as f64, pos.y as f64))) } else { None })
        .collect()
}

fn transform_points(points: &[Point2<f64>], camera_intrinsics: &RosOpenCvIntrinsics<f32>) -> Vec<Point2<f64>> {
    let scaled_points = points.iter().map(|p| Point2::new(p.x / 4095. * 98., p.y / 4095. * 98.)).collect::<Vec<_>>();
    let undistorted_points = ats_cv::undistort_points(&ats_cv::ros_opencv_intrinsics_type_convert(camera_intrinsics), &scaled_points);
    undistorted_points.iter().map(|p| Point2::new(p.x / 98. * 4095., p.y / 98. * 4095.)).collect()
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
