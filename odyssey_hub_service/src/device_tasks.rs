use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use ats_cv::kalman::Pva2d;
use ats_usb::device::UsbDevice;
use ats_usb::packet::CombinedMarkersReport;
use nalgebra::Point2;
use odyssey_hub_common::device::{CdcDevice, Device, UdpDevice};
use opencv_ros_camera::RosOpenCvIntrinsics;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::Sender;
use tokio_stream::StreamExt;

static RECV_PORT: u16 = 23457;

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
    let multicast_port = 23456;
    let multicast = Ipv4Addr::new(224, 0, 2, 52);
    let multicast_addr = SocketAddrV4::new(multicast, multicast_port);

    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP)).unwrap();
    let addr = std::net::SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    socket.set_nonblocking(true).unwrap();
    socket.set_reuse_address(true).unwrap();
    socket.bind(&socket2::SockAddr::from(addr)).unwrap();
    let socket = UdpSocket::from_std(socket.into()).unwrap();
    socket.join_multicast_v4(multicast, Ipv4Addr::UNSPECIFIED).unwrap();
    socket.set_multicast_ttl_v4(5).unwrap();

    let stream_task_handles = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let old_list = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    loop {
        let new_list = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let stream_task_handles = stream_task_handles.clone();
        let mut buf = [0; 1472];
        socket.send_to(&[255, 1], multicast_addr).await.unwrap();
        futures::future::select(
            std::pin::pin!(tokio::time::sleep(tokio::time::Duration::from_secs(2))),
            std::pin::pin!(async {
                let old_list = old_list.clone();
                let new_list = new_list.clone();
                let message_channel = message_channel.clone();
                let stream_task_handles = stream_task_handles.clone(); // Clone the Arc
                loop {
                    let (_len, addr) = socket.recv_from(&mut buf).await.unwrap();
                    if buf[0] == 255 || buf[1] != 1 /* Ping */ { continue; }
                    let device = odyssey_hub_common::device::UdpDevice { id: buf[1], addr, uuid: [0; 6]};
                    let old_list = old_list.lock().await;
                    if !old_list.contains(&device) {
                        let _ = message_channel.send(Message::Connect(odyssey_hub_common::device::Device::Udp(device.clone()))).await;
                        stream_task_handles.lock().await.push((
                            device.clone(),
                            tokio::spawn({
                                let stream_task_handles = stream_task_handles.clone();
                                let message_channel = message_channel.clone();
                                async move {
                                    match device_udp_stream_task(device.clone(), message_channel).await {
                                        Ok(_) => {},
                                        Err(e) => {
                                            eprintln!("Error in device stream task: {}", e);
                                        }
                                    }
                                    stream_task_handles.lock().await.retain(|&(ref x, _)| x != &device);
                                }
                            }),
                        ));
                    }
                    new_list.lock().await.push(device);
                }
            })
        ).await;
        // dbg!(&new_list);
        let new_list = new_list.lock().await;
        for v in old_list.lock().await.iter() {
            if !new_list.contains(v) {
                let _ = message_channel.send(Message::Disconnect(Device::Udp(v.clone()))).await;
                let mut stream_task_handles = stream_task_handles.lock().await;
                if let Some((_, handle)) = stream_task_handles.iter().find(|x| x.0 == *v) {
                    handle.abort();
                    stream_task_handles.retain(|x| x.0 != *v);
                }
            }
        }
        *old_list.lock().await = new_list.to_vec();
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

async fn device_udp_stream_task(device: UdpDevice, message_channel: Sender<Message>) -> anyhow::Result<()> {
    let d = match UsbDevice::connect_hub("0.0.0.0:".to_owned() + RECV_PORT.to_string().as_str(), device.addr.to_string().as_str()).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to connect to device {}: {}", device.addr, e);
            return Ok(());
        },
    };

    tracing::info!("Connected to device {}", device.addr);

    let timeout = tokio::time::Duration::from_millis(500);
    let config = retry(|| d.read_config(), timeout, 3).await.ok_or(anyhow::anyhow!("Failed to read config"))??;

    let mut device = device.clone();
    device.uuid = config.uuid.clone();

    let _ = message_channel.send(Message::Connect(odyssey_hub_common::device::Device::Udp(device.clone()))).await;

    let orientation = Arc::new(tokio::sync::Mutex::new(nalgebra::Rotation3::identity()));

    let screen_id = 0;
    let mut nf_pva2ds: [Pva2d<f64>; 16] = Default::default();
    let mut wf_pva2ds: [Pva2d<f64>; 16] = Default::default();

    let combined_markers_task = {
        let d = d.clone();
        let message_channel = message_channel.clone();
        let orientation = orientation.clone();
        async move {
            let mut stream = d.stream_combined_markers().await.unwrap();

            while let Some(combined_markers) = stream.next().await {
                let CombinedMarkersReport { timestamp, nf_points, wf_points, nf_radii, wf_radii } = combined_markers;

                let mut aim_point = None;
                let mut rotation_mat = None;
                let mut translation_mat = None;

                let filtered_nf_point_tuples = filter_and_create_point_id_tuples(&nf_points, &nf_radii);
                let filtered_wf_point_tuples = filter_and_create_point_id_tuples(&wf_points, &wf_radii);
    
                let filtered_nf_points_slice = filtered_nf_point_tuples.iter().map(|(_, p)| *p).collect::<Vec<_>>();
                let filtered_wf_points_slice = filtered_wf_point_tuples.iter().map(|(_, p)| *p).collect::<Vec<_>>();
    
                let mut nf_points_transformed = transform_points(&filtered_nf_points_slice, &config.camera_model_nf);
                let mut wf_points_transformed = transform_points(&filtered_wf_points_slice, &config.camera_model_wf);
    
                let nf_point_tuples_transformed = filtered_nf_point_tuples.iter().map(|(id, _)| *id).zip(&mut nf_points_transformed).collect::<Vec<_>>();
                let wf_point_tuples_transformed = filtered_wf_point_tuples.iter().map(|(id, _)| *id).zip(&mut wf_points_transformed).collect::<Vec<_>>();

                fn update_positions(pva2ds: &mut [Pva2d<f64>], points: Vec<(usize, &mut Point2<f64>)>) {
                    for (i, point) in points {
                        pva2ds[i].step();
                        pva2ds[i].observe(point.coords.as_ref(), &[100.0, 100.0]);
                        point.x = pva2ds[i].position()[0];
                        point.y = pva2ds[i].position()[1];
                    }
                }
    
                update_positions(&mut nf_pva2ds, nf_point_tuples_transformed);
                update_positions(&mut wf_pva2ds, wf_point_tuples_transformed);
    
                let (rotation, translation, fv_aim_point) = ats_cv::get_pose_and_aimpoint_foveated(
                    &nf_points_transformed,
                    &wf_points_transformed,
                    *orientation.lock().await,
                    screen_id,
                    &ats_cv::ros_opencv_intrinsics_type_convert(&config.camera_model_nf),
                    &ats_cv::ros_opencv_intrinsics_type_convert(&config.camera_model_wf),
                    config.stereo_iso.cast(),
                );
                if let Some(rotation) = rotation {
                    rotation_mat = Some(rotation);
                }
                if let Some(translation) = translation {
                    translation_mat = Some(translation);
                }
                if let Some(fv_aim_point) = fv_aim_point {
                    aim_point = Some(fv_aim_point);
                }
    
                if let Some(aim_point) = aim_point {
                    let aim_point_matrix = nalgebra::Matrix::<f64, nalgebra::Const<2>, nalgebra::Const<1>, nalgebra::ArrayStorage<f64, 2, 1>>::from_column_slice(&[aim_point.x, aim_point.y]);
                    let _ = message_channel.send(Message::Event(odyssey_hub_common::events::Event::DeviceEvent(
                        odyssey_hub_common::events::DeviceEvent {
                            device: Device::Udp(device.clone()),
                            kind: odyssey_hub_common::events::DeviceEventKind::TrackingEvent(odyssey_hub_common::events::TrackingEvent {
                                timestamp,
                                aimpoint: aim_point_matrix,
                                pose: {
                                    if let (Some(rotation_mat), Some(translation_mat)) = (rotation_mat, translation_mat) {
                                        Some(odyssey_hub_common::events::Pose {
                                            rotation: rotation_mat,
                                            translation: translation_mat,
                                        })
                                    } else {
                                        None
                                    }
                                },
                                screen_id: screen_id as u32,
                            }),
                        }
                    ))).await;
                }
            }
        }
    };

    let euler_task = {
        let d = d.clone();
        async move {
            let mut euler_stream = d.stream_euler_angles().await.unwrap();
            while let Some(_orientation) = euler_stream.next().await {
                *orientation.lock().await = _orientation.rotation;
            }
        }
    };

    let impact_task = {
        let d = d.clone();
        let message_channel = message_channel.clone();
        async move {
            let mut stream = d.stream_impact().await.unwrap();

            while let Some(impact) = stream.next().await {
                let _ = message_channel.send(Message::Event(odyssey_hub_common::events::Event::DeviceEvent(
                    odyssey_hub_common::events::DeviceEvent {
                        device: Device::Udp(device.clone()),
                        kind: odyssey_hub_common::events::DeviceEventKind::ImpactEvent(odyssey_hub_common::events::ImpactEvent {
                            timestamp: impact.timestamp,
                        }),
                    }
                ))).await;
            }
        }
    };

    tokio::select! {
        _ = combined_markers_task => {},
        _ = euler_task => {},
        _ = impact_task => {},
    }

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

    let screen_id = 0;
    let mut nf_pva2ds: [Pva2d<f64>; 16] = Default::default();
    let mut wf_pva2ds: [Pva2d<f64>; 16] = Default::default();

    let combined_markers_task = {
        let d = d.clone();
        let device = device.clone();
        let message_channel = message_channel.clone();
        let orientation = orientation.clone();
        async move {
            let mut stream = d.stream_combined_markers().await.unwrap();

            while let Some(combined_markers) = stream.next().await {
                let CombinedMarkersReport { timestamp, nf_points, wf_points, nf_radii, wf_radii } = combined_markers;

                let mut aim_point = None;
                let mut rotation_mat = None;
                let mut translation_mat = None;

                let filtered_nf_point_tuples = filter_and_create_point_id_tuples(&nf_points, &nf_radii);
                let filtered_wf_point_tuples = filter_and_create_point_id_tuples(&wf_points, &wf_radii);
    
                let filtered_nf_points_slice = filtered_nf_point_tuples.iter().map(|(_, p)| *p).collect::<Vec<_>>();
                let filtered_wf_points_slice = filtered_wf_point_tuples.iter().map(|(_, p)| *p).collect::<Vec<_>>();
    
                let mut nf_points_transformed = transform_points(&filtered_nf_points_slice, &config.camera_model_nf);
                let mut wf_points_transformed = transform_points(&filtered_wf_points_slice, &config.camera_model_wf);
    
                let nf_point_tuples_transformed = filtered_nf_point_tuples.iter().map(|(id, _)| *id).zip(&mut nf_points_transformed).collect::<Vec<_>>();
                let wf_point_tuples_transformed = filtered_wf_point_tuples.iter().map(|(id, _)| *id).zip(&mut wf_points_transformed).collect::<Vec<_>>();

                fn update_positions(pva2ds: &mut [Pva2d<f64>], points: Vec<(usize, &mut Point2<f64>)>) {
                    for (i, point) in points {
                        pva2ds[i].step();
                        pva2ds[i].observe(point.coords.as_ref(), &[100.0, 100.0]);
                        point.x = pva2ds[i].position()[0];
                        point.y = pva2ds[i].position()[1];
                    }
                }
    
                update_positions(&mut nf_pva2ds, nf_point_tuples_transformed);
                update_positions(&mut wf_pva2ds, wf_point_tuples_transformed);
    
                let (rotation, translation, fv_aim_point) = ats_cv::get_pose_and_aimpoint_foveated(
                    &nf_points_transformed,
                    &wf_points_transformed,
                    *orientation.lock().await,
                    screen_id,
                    &ats_cv::ros_opencv_intrinsics_type_convert(&config.camera_model_nf),
                    &ats_cv::ros_opencv_intrinsics_type_convert(&config.camera_model_wf),
                    config.stereo_iso.cast(),
                );
                if let Some(rotation) = rotation {
                    rotation_mat = Some(rotation);
                }
                if let Some(translation) = translation {
                    translation_mat = Some(translation);
                }
                if let Some(fv_aim_point) = fv_aim_point {
                    aim_point = Some(fv_aim_point);
                }
                
                if let Some(aim_point) = aim_point {
                    let aim_point_matrix = nalgebra::Matrix::<f64, nalgebra::Const<2>, nalgebra::Const<1>, nalgebra::ArrayStorage<f64, 2, 1>>::from_column_slice(&[aim_point.x, aim_point.y]);
                    let _ = message_channel.send(Message::Event(odyssey_hub_common::events::Event::DeviceEvent(
                        odyssey_hub_common::events::DeviceEvent {
                            device: Device::Cdc(device.clone()),
                            kind: odyssey_hub_common::events::DeviceEventKind::TrackingEvent(odyssey_hub_common::events::TrackingEvent {
                                timestamp,
                                aimpoint: aim_point_matrix,
                                pose: {
                                    if let (Some(rotation_mat), Some(translation_mat)) = (rotation_mat, translation_mat) {
                                        Some(odyssey_hub_common::events::Pose {
                                            rotation: rotation_mat,
                                            translation: translation_mat,
                                        })
                                    } else {
                                        None
                                    }
                                },
                                screen_id: screen_id as u32,
                            }),
                        }
                    ))).await;
                }
            }
        }
    };

    let euler_task = {
        let d = d.clone();
        async move {
            let mut euler_stream = d.stream_euler_angles().await.unwrap();
            while let Some(_orientation) = euler_stream.next().await {
                *orientation.lock().await = _orientation.rotation;
            }
        }
    };

    let impact_task = {
        let d = d.clone();
        let message_channel = message_channel.clone();
        async move {
            let mut stream = d.stream_impact().await.unwrap();

            while let Some(impact) = stream.next().await {
                let _ = message_channel.send(Message::Event(odyssey_hub_common::events::Event::DeviceEvent(
                    odyssey_hub_common::events::DeviceEvent {
                        device: Device::Cdc(device.clone()),
                        kind: odyssey_hub_common::events::DeviceEventKind::ImpactEvent(odyssey_hub_common::events::ImpactEvent {
                            timestamp: impact.timestamp,
                        }),
                    }
                ))).await;
            }
        }
    };

    tokio::select! {
        _ = combined_markers_task => {},
        _ = euler_task => {},
        _ = impact_task => {},
    }

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
