use std::net::Ipv4Addr;
use ats_cv::kalman::Pva2d;
use ats_usb::device::UsbDevice;
use ats_usb::packet::CombinedMarkersReport;
use nalgebra::{Matrix3, MatrixXx1, MatrixXx2, Point2, Rotation3, Scalar, Translation3, Vector2, Vector3};
use odyssey_hub_common::device::Device;
use opencv_ros_camera::{Distortion, RosOpenCvIntrinsics};
use sqpnp::types::{SQPSolution, SolverParameters};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{self, Sender};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

#[derive(Debug, Clone)]
pub enum Message {
    Connect(odyssey_hub_common::device::Device),
    Disconnect(odyssey_hub_common::device::Device),
    Event(odyssey_hub_common::events::Event),
}

pub async fn device_tasks(message_channel: Sender<Message>) -> anyhow::Result<()> {
    let mut dl_cancellation_pairs = vec![];

    let (sender, mut receiver) = mpsc::channel(12);
    tokio::select! {
        _ = device_udp_ping_task(sender.clone()) => {},
        _ = device_hid_ping_task(sender.clone()) => {},
        _ = device_cdc_ping_task(sender.clone()) => {},
        _ = async {
            while let Some(message) = receiver.recv().await {
                match message.clone() {
                    Message::Connect(d) => {
                        let ct = CancellationToken::new();
                        dl_cancellation_pairs.push((d.clone(), ct.clone()));
                        tokio::spawn(device_stream_task(d.clone(), ct.clone(), sender.clone()));
                    },
                    Message::Disconnect(d) => {
                        let i = dl_cancellation_pairs.iter().position(|a| a.0 == d);
                        if let Some(i) = i {
                            dl_cancellation_pairs[i].1.cancel();
                            dl_cancellation_pairs.remove(i);
                        }
                    },
                    _ => {},
                }
                message_channel.send(message).await.unwrap();
            }
        } => {},
    }
    Ok(())
}

async fn device_udp_ping_task(message_channel: Sender<Message>) -> std::convert::Infallible {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 23456)).await.unwrap();
    socket.set_broadcast(true).unwrap();

    let mut old_list = vec![];
    loop {
        let mut new_list = vec![];
        let mut buf = [0; 1472];
        socket.send_to(&[255, 1], (Ipv4Addr::BROADCAST, 23456)).await.unwrap();
        futures::future::select(
            std::pin::pin!(tokio::time::sleep(tokio::time::Duration::from_secs(2))),
            std::pin::pin!(async {
                loop {
                    let (len, addr) = socket.recv_from(&mut buf).await.unwrap();
                    if buf[0] == 255 { continue; }
                    if !old_list.contains(&odyssey_hub_common::device::Device::Udp(odyssey_hub_common::device::UdpDevice { id: buf[1], addr: addr })) {
                        let _ = message_channel.send(Message::Connect(odyssey_hub_common::device::Device::Udp(odyssey_hub_common::device::UdpDevice { id: buf[1], addr: addr }))).await;
                    }
                    new_list.push(odyssey_hub_common::device::Device::Udp(odyssey_hub_common::device::UdpDevice { id: buf[1], addr: addr }));
                }
            })
        ).await;
        // dbg!(&new_list);
        for v in &old_list {
            if !new_list.contains(v) {
                let _ = message_channel.send(Message::Disconnect(v.clone())).await;
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

async fn device_stream_task(device: Device, ct: CancellationToken, message_channel: Sender<Message>) {
    match device {
        Device::Udp(d) => {
            let d = UsbDevice::connect_hub("0.0.0.0:23456", d.addr.to_string().as_str()).await.unwrap();
            let mut stream = d.stream_combined_markers().await.unwrap();

            let mut nf_pva2ds: [Pva2d<f64>; 4] = Default::default();

            if let Some(combined_markers) = stream.next().await {
                let CombinedMarkersReport { nf_points, wf_points, nf_radii, wf_radii } = combined_markers;
    
                let mut rotation_mat = None;
                let mut translation_mat = None;
                let mut aim_point = None;

                let camera_model_nf = RosOpenCvIntrinsics::from_params_with_distortion(
                    6070.516352962917,     // fx 
                    0.0,
                    6081.704092101642,     // fy 
                    2012.5340172306787,    // cx 
                    2053.6576652187186,    // cy 
                    Distortion::from_opencv_vec(nalgebra::Vector5::new(
                        -0.08717430574570494,  // k1 
                        0.5130932472490415,    // k2 
                        0.0050450411569348645, // p1 
                        0.001854950091801636,  // p2 
                        0.048480928208383456,  // k3 
                    )),
                );

                let nf_points = nf_points.into_iter().zip(nf_radii.into_iter()).filter_map(|(pos, r)| if r > 0 { Some(pos) } else { None }).collect::<Vec<_>>();
                let nf_points = nf_points.into_iter().map(|pos| Point2::new(pos.x as f64, pos.y as f64)).collect::<Vec<_>>();
    
                let x_mat = MatrixXx1::from_column_slice(&nf_points.iter().map(|p| p.x as f64).collect::<Vec<_>>());
                let y_mat = MatrixXx1::from_column_slice(&nf_points.iter().map(|p| p.y as f64).collect::<Vec<_>>());
                let points = MatrixXx2::from_columns(&[x_mat, y_mat]);
                let distorted = cam_geom::Pixels::new(points);
                let undistorted = camera_model_nf.undistort(&distorted);
    
                let x_vec = undistorted.data.as_slice()[..undistorted.data.len() / 2].to_vec();
                let y_vec = undistorted.data.as_slice()[undistorted.data.len() / 2..].to_vec();
                let mut nf_points = x_vec.into_iter().zip(y_vec).map(|(x, y)| Point2::new(x, y)).collect::<Vec<_>>();
    
                if nf_points.len() > 3 {
                    for (i, pva2d) in nf_pva2ds.iter_mut().enumerate() {
                        pva2d.step();
                        pva2d.observe(nf_points[i].coords.as_ref(), &[100.0, 100.0]);
                        nf_points[i].x = pva2d.position()[0];
                        nf_points[i].y = pva2d.position()[1];
                    }
                }

                /// Given 4 points in the following shape
                ///
                /// ```
                /// +--x
                /// |
                /// y        a              b
                ///
                ///
                ///          c              d
                /// ```
                ///
                /// Sort them into the order a, b, d, c
                pub fn sort_rectangle<T: Scalar + PartialOrd>(a: &mut [Point2<T>]) {
                    if a[0].y > a[2].y { a.swap(0, 2); }
                    if a[1].y > a[3].y { a.swap(1, 3); }
                    if a[0].y > a[1].y { a.swap(0, 1); }
                    if a[2].y > a[3].y { a.swap(2, 3); }
                    if a[1].y > a[2].y { a.swap(1, 2); }
                    if a[0].x > a[1].x { a.swap(0, 1); }
                    if a[2].x < a[3].x { a.swap(2, 3); }
                }

                pub fn my_pnp(projections: &[Vector2<f64>]) -> Option<SQPSolution> {
                    let _3dpoints = [
                        Vector3::new((0.35 - 0.5) * 16./9., -0.5, 0.),
                        Vector3::new((0.65 - 0.5) * 16./9., -0.5, 0.),
                        Vector3::new((0.65 - 0.5) * 16./9., 0.5, 0.),
                        Vector3::new((0.35 - 0.5) * 16./9., 0.5, 0.),
                    ];
                    let solver = sqpnp::PnpSolver::new(&_3dpoints, &projections, None, SolverParameters::default());
                    if let Some(mut solver) = solver {
                        solver.solve();
                        debug!("pnp found {} solutions", solver.number_of_solutions());
                        if solver.number_of_solutions() >= 1 {
                            return Some(solver.solution_ptr(0).unwrap().clone());
                        }
                    } else {
                        info!("pnp solver failed");
                    }
                    None
                }

                fn ray_plane_intersection(ray_origin: Vector3<f64>, ray_direction: Vector3<f64>, plane_normal: Vector3<f64>, plane_point: Vector3<f64>) -> Vector3<f64> {
                    let d = plane_normal.dot(&plane_point);
                    let t = (d - plane_normal.dot(&ray_origin)) / plane_normal.dot(&ray_direction);
                    ray_origin + t * ray_direction
                }

                aim_point = if nf_points.len() > 3 {
                    let mut nf_positions = nf_points.clone();
                    sort_rectangle(&mut nf_positions);
                    let center_aim = Point2::new(2047.5, 2047.5);
                    let projections = nf_positions.iter().map(|pos| {
                        // 1/math.tan(38.3 / 180 * math.pi / 2) * 2047.5 (value used in the sim)
                        let f = 5896.181431117499;
                        let x = (pos.x - 2047.5) / f;
                        let y = (pos.y - 2047.5) / f;
                        Vector2::new(x, y)
                    }).collect::<Vec<Vector2<_>>>();
                    let solution = my_pnp(&projections);
                    if let Some(solution) = solution {
                        let r_hat = Rotation3::from_matrix_unchecked(solution.r_hat.reshape_generic(nalgebra::Const::<3>, nalgebra::Const::<3>).transpose());
                        let t = Translation3::from(solution.t);
                        let tf = t * r_hat;
                        let ctf = tf.inverse();
    
                        let flip_yz = Matrix3::new(
                            1., 0., 0.,
                            0., -1., 0.,
                            0., 0., -1.,
                        );

                        let rotmat = flip_yz * ctf.rotation.matrix() * flip_yz;
                        let transmat = flip_yz * ctf.translation.vector;

                        rotation_mat = Some(flip_yz * ctf.rotation.matrix() * flip_yz);
                        translation_mat = Some(flip_yz * ctf.translation.vector);
    
                        let screen_3dpoints = [
                            Vector3::new((0.0 - 0.5) * 16./9., -0.5, 0.),
                            Vector3::new((1.0 - 0.5) * 16./9., -0.5, 0.),
                            Vector3::new((1.0 - 0.5) * 16./9., 0.5, 0.),
                            Vector3::new((0.0 - 0.5) * 16./9., 0.5, 0.),
                        ];
    
                        let ray_origin = transmat;
                        let ray_direction = rotmat * Vector3::new(0., 0., 1.);
                        let plane_normal = (screen_3dpoints[1] - screen_3dpoints[0]).cross(&(screen_3dpoints[2] - screen_3dpoints[0]));
                        let plane_point = screen_3dpoints[0];
                        let _aim_point = ray_plane_intersection(ray_origin, ray_direction, plane_normal, plane_point);
    
                        let _aim_point = Point2::new(
                            (_aim_point.x - screen_3dpoints[0].x) / (screen_3dpoints[2].x - screen_3dpoints[0].x),
                            (_aim_point.y - screen_3dpoints[0].y) / (screen_3dpoints[2].y - screen_3dpoints[0].y),
                        );
    
                        let _aim_point = Point2::new(_aim_point.x, 1. - _aim_point.y);
                        Some(_aim_point)
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some(aim_point) = aim_point {
                    let aim_point_matrix = nalgebra::Matrix::<f64, nalgebra::Const<2>, nalgebra::Const<1>, nalgebra::ArrayStorage<f64, 2, 1>>::from_column_slice(&[aim_point.x, aim_point.y]);
                    let _ = message_channel.send(Message::Event(odyssey_hub_common::events::Event::DeviceEvent(
                        odyssey_hub_common::events::DeviceEvent {
                            device: device.clone(),
                            kind: odyssey_hub_common::events::DeviceEventKind::TrackingEvent(odyssey_hub_common::events::TrackingEvent {
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
        },
        Device::Hid(d) => {},
        Device::Cdc(d) => {},
    }
}
