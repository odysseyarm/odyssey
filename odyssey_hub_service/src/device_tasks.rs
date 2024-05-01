use ahrs::Ahrs;
use std::net::Ipv4Addr;
use std::sync::Arc;
use ats_cv::kalman::Pva2d;
use ats_usb::device::UsbDevice;
use ats_usb::packet::CombinedMarkersReport;
use nalgebra::{Const, Isometry3, Matrix3, MatrixXx1, MatrixXx2, Point2, Rotation3, Scalar, Translation3, Vector2, Vector3};
use odyssey_hub_common::device::{Device, UdpDevice};
use opencv_ros_camera::{Distortion, RosOpenCvIntrinsics};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::Sender;
use tokio_stream::StreamExt;

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
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await.unwrap();
    socket.set_broadcast(true).unwrap();

    let mut stream_task_handles = vec![];
    let mut old_list = vec![];
    loop {
        let mut new_list = vec![];
        let mut buf = [0; 1472];
        socket.send_to(&[255, 1], (Ipv4Addr::BROADCAST, 23456)).await.unwrap();
        futures::future::select(
            std::pin::pin!(tokio::time::sleep(tokio::time::Duration::from_secs(2))),
            std::pin::pin!(async {
                loop {
                    let (_len, addr) = socket.recv_from(&mut buf).await.unwrap();
                    if buf[0] == 255 || buf[1] != 1 /* Ping */ { continue; }
                    let device = odyssey_hub_common::device::UdpDevice { id: buf[1], addr };
                    if !old_list.contains(&device) {
                        let _ = message_channel.send(Message::Connect(odyssey_hub_common::device::Device::Udp(device.clone()))).await;
                        stream_task_handles.push((
                            device.clone(),
                            tokio::spawn(device_udp_stream_task(device.clone(), message_channel.clone())),
                        ));
                    }
                    new_list.push(device);
                }
            })
        ).await;
        // dbg!(&new_list);
        for v in &old_list {
            if !new_list.contains(v) {
                let _ = message_channel.send(Message::Disconnect(Device::Udp(v.clone()))).await;
                if let Some((_, handle)) = stream_task_handles.iter().find(|x| x.0 == *v) {
                    handle.abort();
                }
            }
        }
        old_list = new_list;
    }
}

async fn device_hid_ping_task(message_channel: Sender<Message>) -> std::convert::Infallible {
    let api = hidapi::HidApi::new().unwrap();

    let mut old_list = vec![];
    loop {
        let mut new_list = vec![];
        for device in api.device_list() {
            if device.vendor_id() == 0x1915 && device.product_id() == 0x48AB {
                if !old_list.contains(&odyssey_hub_common::device::Device::Hid(odyssey_hub_common::device::HidDevice { path: device.path().to_str().unwrap().to_string() })) {
                    let _ = message_channel.send(Message::Connect(odyssey_hub_common::device::Device::Hid(odyssey_hub_common::device::HidDevice { path: device.path().to_str().unwrap().to_string() }))).await;
                }
                new_list.push(odyssey_hub_common::device::Device::Hid(odyssey_hub_common::device::HidDevice { path: device.path().to_str().unwrap().to_string() }));
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
    let mut old_list = vec![];
    loop {
        let mut new_list = vec![];
        let ports = serialport::available_ports();
        let ports: Vec<_> = match ports {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to list serial ports {}", &e.to_string());
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                continue;
            }
        }.into_iter().filter(|port| {
            match &port.port_type {
                serialport::SerialPortType::UsbPort(port_info) => {
                    if port_info.vid != 0x1915 || port_info.pid != 0x520f {
                        return false;
                    }
                    if let Some(i) = port_info.interface {
                        // interface 0: cdc acm module
                        // interface 1: cdc acm module functional subordinate interface
                        // interface 2: cdc acm dfu
                        // interface 3: cdc acm dfu subordinate interface
                        i == 0
                    } else {
                        true
                    }
                },
                _ => false,
            }
        }).collect();
        for device in ports {
            if !old_list.contains(&odyssey_hub_common::device::Device::Cdc(odyssey_hub_common::device::CdcDevice { path: device.port_name.clone() })) {
                let _ = message_channel.send(Message::Connect(odyssey_hub_common::device::Device::Cdc(odyssey_hub_common::device::CdcDevice { path: device.port_name.clone() }))).await;
            }
            new_list.push(odyssey_hub_common::device::Device::Cdc(odyssey_hub_common::device::CdcDevice { path: device.port_name.clone() }));
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

async fn device_udp_stream_task(device: UdpDevice, message_channel: Sender<Message>) {
    let d = UsbDevice::connect_hub("0.0.0.0:0", device.addr.to_string().as_str()).await.unwrap();

    // todo get odr
    // let mut orientation = ahrs::Madgwick::new(1./400., 0.1);

    let gravity_angle = Arc::new(tokio::sync::Mutex::new(0.));

    let screen_id = 0;
    let mut nf_pva2ds: [Pva2d<f64>; 16] = Default::default();
    let mut wf_pva2ds: [Pva2d<f64>; 16] = Default::default();

    // todo modules will have their own calibration data
    let nf_default_yaml = b"
        camera_matrix: !!opencv-matrix
            rows: 3
            cols: 3
            dt: d
            data: [ 145.10303635182407, 0., 47.917007513463489, 0.,
                145.29149528441428, 49.399597700110256, 0., 0., 1. ]
        dist_coeffs: !!opencv-matrix
            rows: 1
            cols: 5
            dt: d
            data: [ -0.20386528104463086, 2.2361997667805928,
                0.0058579118546963271, -0.0013804251043507982,
                -7.7712738787306455 ]
        rms_error: 0.074045711900311811
        num_captures: 62
    ";

    let wf_default_yaml = b"
        camera_matrix: !!opencv-matrix
            rows: 3
            cols: 3
            dt: d
            data: [ 34.34121647200962, 0., 48.738547789766642, 0.,
                34.394866762375322, 49.988446965249153, 0., 0., 1. ]
        dist_coeffs: !!opencv-matrix
            rows: 1
            cols: 5
            dt: d
            data: [ 0.039820534469617412, -0.039933169314557031,
                0.00043006078813049756, -0.0012057066028621883,
                0.0053022349797757964 ]
        rms_error: 0.066816050332039037
        num_captures: 64
    ";

    let stereo_default_yaml = b"
        r: !!opencv-matrix
            rows: 3
            cols: 3
            dt: d
            data: [ 0.99998692169365289, -0.0051111539633757353,
                -0.00018040735794555248, 0.0050780667355570033,
                0.99647084468055069, -0.083785851668757322,
                0.00060801306019016873, 0.083783839771118418, 0.99648376731049981 ]
        t: !!opencv-matrix
            rows: 3
            cols: 1
            dt: d
            data: [ -0.011673356870756095, -0.33280456937540659,
                0.19656043337961257 ]
        rms_error: 0.1771451374078685
        num_captures: 49
    ";

    let camera_model_nf = ats_cv::get_intrinsics_from_opencv_camera_calibration_yaml(&nf_default_yaml[..]).unwrap();
    let camera_model_wf = ats_cv::get_intrinsics_from_opencv_camera_calibration_yaml(&wf_default_yaml[..]).unwrap();
    let stereo_iso = ats_cv::get_isometry_from_opencv_stereo_calibration_yaml(&stereo_default_yaml[..]).unwrap();

    let combined_markers_task = {
        let d = d.clone();
        let message_channel = message_channel.clone();
        let gravity_angle = gravity_angle.clone();
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
    
                let mut nf_points_transformed = transform_points(&filtered_nf_points_slice, &camera_model_nf);
                let mut wf_points_transformed = transform_points(&filtered_wf_points_slice, &camera_model_wf);
    
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
                    *gravity_angle.lock().await,
                    screen_id,
                    &camera_model_nf,
                    &camera_model_wf,
                    stereo_iso
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
                                }
                            }),
                        }
                    ))).await;
                }
            }
        }
    };

    let accel_task = {
        let d = d.clone();
        let gravity_angle = gravity_angle.clone();
        async move {
            let mut stream = d.stream_accel().await.unwrap();
            while let Some(accel) = stream.next().await {
                // let _ = orientation.update_imu(&Vector3::from(accel.gyro), &Vector3::from(accel.accel));
                *gravity_angle.lock().await = accel.gravity_angle as f64;
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
        _ = accel_task => {},
        _ = impact_task => {},
    }
}

fn filter_and_create_point_id_tuples(points: &[Point2<u16>], radii: &[u8]) -> Vec<(usize, Point2<f64>)> {
    points.iter().zip(radii.iter())
        .enumerate()
        .filter_map(|(id, (pos, &r))| if r > 0 { Some((id, Point2::new(pos.x as f64, pos.y as f64))) } else { None })
        .collect()
}

fn transform_points(points: &[Point2<f64>], camera_intrinsics: &RosOpenCvIntrinsics<f64>) -> Vec<Point2<f64>> {
    let scaled_points = points.iter().map(|p| Point2::new(p.x / 4095. * 98., p.y / 4095. * 98.)).collect::<Vec<_>>();
    let undistorted_points = ats_cv::undistort_points(camera_intrinsics, &scaled_points);
    undistorted_points.iter().map(|p| Point2::new(p.x / 98. * 4095., p.y / 98. * 4095.)).collect()
}
