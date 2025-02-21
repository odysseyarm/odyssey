use std::{
    fmt::Display,
    fs::File,
    io::BufWriter,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU16, Ordering}, mpsc::Receiver, Arc, Mutex
    },
    time::{Duration, Instant},
};

use draw_pattern_points::DrawPatternPoints;
use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
use mot_data::MotData;
use opencv::core::{FileStorage, FileStorageTrait, FileStorageTraitConst, FileStorage_FORMAT_JSON, FileStorage_WRITE, Mat, Point2f, Vector};
use serialport::{ClearBuffer, SerialPort};

pub mod mot_data;
pub mod charuco;
pub mod chessboard;
pub mod circles;
pub mod draw_pattern_points;

use macroquad::prelude::*;

const CALIBRATION_VERSION: i32 = 1;
const PROTOCOL_VERSION: u32 = 1;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Port {
    Wf,
    Nf,
}

struct PajData {
    image: Vec<u8>,
    gray: Vec<u8>,
    mot_data: [MotData; 16],
    pattern_points: Option<Vec<Point2f>>,
    avg_frame_period: f64,
    last_frame: Instant,
}

impl Default for PajData {
    fn default() -> Self {
        Self {
            image: vec![],
            gray: vec![],
            mot_data: Default::default(),
            avg_frame_period: 1.0,
            last_frame: Instant::now(),
            pattern_points: None,
        }
    }
}

struct MainState {
    wf_data: Arc<Mutex<PajData>>,
    nf_data: Arc<Mutex<PajData>>,
    capture_count: usize,
    captured_nf: Vec<Vec<u8>>,
    captured_wf: Vec<Vec<u8>>,
    captured_nf_files: Vec<String>,
    captured_wf_files: Vec<String>,
    detector_params: Arc<DetectorParams>,
    reset: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
    draw_pattern_points: DrawPatternPoints,
    device_uuid: DeviceUuid,
    resolution: (u16, u16),
    object_resolution: (u32, u32),
}

impl MainState {
    fn add_capture(&mut self, port: Port, image: Vec<u8>, path: String) {
        match port {
            Port::Wf => {
                self.captured_wf.push(image);
                self.captured_wf_files.push(path);
            }
            Port::Nf => {
                self.captured_nf.push(image);
                self.captured_nf_files.push(path);
            }
        }
    }
    fn clear_captures(&mut self) {
        self.captured_wf.clear();
        self.captured_nf.clear();
        self.captured_wf_files.clear();
        self.captured_nf_files.clear();
    }
}

#[derive(Debug)]
struct DetectorParams {
    rows: AtomicU16,
    cols: AtomicU16,
    pattern: atomic::Atomic<DetectorPattern>,
    special: AtomicBool,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, bytemuck::NoUninit)]
#[repr(u8)]
enum DetectorPattern {
    None,
    Chessboard,
    AsymmetricCircles,
    SymmetricCircles,
}

impl Display for DetectorPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DetectorPattern::None => write!(f, "off"),
            DetectorPattern::Chessboard => write!(f, "chessboard"),
            DetectorPattern::AsymmetricCircles => write!(f, "acircles"),
            DetectorPattern::SymmetricCircles => write!(f, "circles grid"),
        }
    }
}

impl MainState {
    fn new(
        capture_count: usize,
        port: Box<dyn SerialPort>,
        device_uuid: DeviceUuid,
        resolution: (u16, u16),
        object_resolution: (u32, u32),
    ) -> MainState {
        let wf_data = Arc::new(Mutex::new(Default::default()));
        let nf_data = Arc::new(Mutex::new(Default::default()));
        let reset = Arc::new(AtomicBool::new(false));
        let quit = Arc::new(AtomicBool::new(false));

        let main_state = MainState {
            wf_data: wf_data.clone(),
            nf_data: nf_data.clone(),
            device_uuid,
            resolution,
            object_resolution,
            capture_count,
            captured_nf: vec![],
            captured_wf: vec![],
            captured_nf_files: vec![],
            captured_wf_files: vec![],
            detector_params: Arc::new(DetectorParams {
                rows: 6.into(),
                cols: 6.into(),
                pattern: atomic::Atomic::new(DetectorPattern::Chessboard),
                special: false.into(),
            }),
            reset: reset.clone(),
            quit: quit.clone(),
            draw_pattern_points: DrawPatternPoints::new(),
        };

        let detector_params = main_state.detector_params.clone();
        // hack-ish
        if resolution == (98, 98) {
            std::thread::spawn(move || {
                paj_reader_thread(
                    port,
                    object_resolution,
                    wf_data,
                    nf_data,
                    detector_params,
                    reset,
                    quit,
                );
            });
        } else if resolution == (320, 240) {
            std::thread::spawn(move || {
                image_only_reader_thread(
                    port,
                    resolution,
                    object_resolution,
                    wf_data,
                    nf_data,
                    detector_params,
                    reset,
                    quit,
                );
            });
        } else {
            panic!("Unsupported resolution");
        }

        main_state
    }

    fn update(&mut self) {
        if self.quit.load(Ordering::Relaxed) {
            std::process::exit(0);
        }
    }

    fn draw(&mut self) {
        clear_background(BLACK);

        let wf_data = self.wf_data.lock().unwrap();
        let nf_data = self.nf_data.lock().unwrap();

        // Image scale factor
        let image_scale = if self.resolution == (98, 98) {
            5.0
        } else {
            2.0
        };
        let rx = self.resolution.0 as f32;
        let ry = self.resolution.1 as f32;
        let image_width = rx * image_scale;
        let image_height = ry * image_scale;
        let spacing = 20.0;

        // Draw images
        if !wf_data.image.is_empty() {
            draw_image(
                &wf_data.image,
                self.resolution,
                vec2(0.0, 0.0),
                image_scale,
            );
        }
        if !nf_data.image.is_empty() {
            draw_image(
                &nf_data.image,
                self.resolution,
                vec2(image_width + spacing, 0.0),
                image_scale,
            );
        }

        // Draw FPS
        let fps = 1.0 / wf_data.avg_frame_period;
        draw_text(&format!("{:.1} fps", fps), 10.0, image_height + 20.0, 20.0, WHITE);
        draw_text(
            &format!("{:.1} fps", fps),
            image_width + spacing + 10.0,
            image_height + 20.0,
            20.0,
            WHITE,
        );

        let cols = self.detector_params.cols.load(Ordering::Relaxed);
        let rows = self.detector_params.rows.load(Ordering::Relaxed);
        let special = self.detector_params.special.load(Ordering::Relaxed);
        let pat = self.detector_params.pattern.load(Ordering::Relaxed);
        draw_text(
            &format!(
                "Captures (space): {}    Board (r, c, s): {rows}x{cols}x{special}    Detector (d): {pat}",
                self.captured_wf.len()
            ),
            10.0,
            image_height + 40.0,
            20.0,
            WHITE,
        );
        draw_text(
            "<1> calib nf    <2> calib wf    <3> stereo calib    <Backspace> clear    <Esc> close",
            10.0,
            image_height + 60.0,
            20.0,
            WHITE,
        );
        draw_text(
            &format!("Device ID: {}", self.device_uuid),
            image_width*2.0-180.0,
            image_height + 40.0,
            20.0,
            WHITE,
        );

        let wf_transform = draw_pattern_points::get_transform_matrix(
            vec2(image_scale, image_scale),
            0.,
            vec2(0., 0.),
            vec2(0., 0.),
        );
        let nf_transform = draw_pattern_points::get_transform_matrix(
            vec2(image_scale, image_scale),
            0.,
            vec2(image_width + spacing, 0.0),
            vec2(0., 0.),
        );

        // Draw circles
        // fixme will look backwards for ats lite
        draw_mot_data_circles(&wf_data.mot_data, self.object_resolution, wf_transform, true);
        draw_mot_data_circles(&nf_data.mot_data, self.object_resolution, nf_transform, false);

        // Draw pattern points if available
        if let Some(pattern_points) = &wf_data.pattern_points {
            self.draw_pattern_points.draw(
                pattern_points,
                cols.into(),
                true,
                self.resolution,
                self.object_resolution,
                wf_transform,
            );
        }
        if let Some(pattern_points) = &nf_data.pattern_points {
            self.draw_pattern_points.draw(
                pattern_points,
                cols.into(),
                true,
                self.resolution,
                self.object_resolution,
                nf_transform,
            );
        }
    }
}

fn draw_image(
    image: &[u8],
    resolution: (u16, u16),
    offset: Vec2,
    scale: f32,
) {
    let texture = Texture2D::from_rgba8(resolution.0, resolution.1, image);
    texture.set_filter(FilterMode::Nearest);
    draw_texture_ex(
        &texture,
        offset.x,
        offset.y,
        WHITE,
        DrawTextureParams {
            dest_size: Some((resolution.0 as f32 * scale, resolution.1 as f32 * scale).into()),
            ..Default::default()
        },
    );
}

fn draw_mot_data_circles(
    mot_data: &[MotData],
    object_resolution: (u32, u32),
    transform: Mat4,
    flip_x: bool,
) {
    // The PAG doesn't seem to support getting both object info and image data simultaneously...?
    for data in mot_data {
        if data.area > 0 {
            let mut point = vec3(
                data.cx as f32 / object_resolution.0 as f32 * 97. + 0.5,
                data.cy as f32 / object_resolution.1 as f32 * 97. + 0.5,
                0.0,
            );
            if flip_x {
                point.x = 98.0 - point.x;
            }
            let transformed_point = transform.transform_point3(point);
            draw_circle_lines(transformed_point.x, transformed_point.y, 4.0, 1.0, RED);
        }
    }
}

pub async fn main() {
    let path = std::env::args().nth(1).unwrap_or("/dev/ttyACM1".into());
    let mut port = serialport::new(path, 115200)
        .timeout(Duration::from_secs(3))
        .open()
        .unwrap();
    let drained = drain(&mut *port);
    if drained > 0 {
        eprintln!("drained {drained} bytes");
    }

    // read device uuid
    port.write(b"i").unwrap();
    port.flush().unwrap();
    let mut version = [0; 4];
    port.read_exact(&mut version).unwrap();
    let version = u32::from_le_bytes(version);
    if version != PROTOCOL_VERSION {
        eprintln!("Protocol version mismatch!");
        if version > 100 {
            // Probably not a version number and is just old firmware returning the device uuid.
            eprintln!("Device did not respond with a valid protocol version");
            eprintln!("Please update the firmware");
        } else {
            eprintln!("Device protocol version {version} does not match expected protocol version {PROTOCOL_VERSION}");
        }
        return;
    }
    let mut device_uuid = DeviceUuid([0; 6]);
    port.read_exact(&mut device_uuid.0).unwrap();
    let mut r = [0; 12];
    port.read_exact(&mut r).unwrap();
    let resolution = (
        u16::from_le_bytes([r[0], r[1]]),
        u16::from_le_bytes([r[2], r[3]]),
    );
    let object_resolution = (
        u32::from_le_bytes([r[4], r[5], r[6], r[7]]),
        u32::from_le_bytes([r[8], r[9], r[10], r[11]]),
    );
    eprintln!("Resolution {}x{}", resolution.0, resolution.1);
    eprintln!("Object resolution {}x{}", object_resolution.0, object_resolution.1);

    std::fs::create_dir_all(format!("calibrations/{device_uuid}/images")).unwrap();
    let capture_count = (0..)
        .find(|i| !Path::new(&format!("calibrations/{device_uuid}/images/nearfield_{:02}.png", i)).exists())
        .unwrap();

    let mut state = MainState::new(capture_count, port, device_uuid, resolution, object_resolution);

    loop {
        state.update();
        state.draw();
        handle_input(&mut state).await;
        next_frame().await;
    }
}

async fn handle_input(state: &mut MainState) {
    let device_uuid = state.device_uuid;
    if is_key_pressed(KeyCode::Escape) {
        state.quit.store(true, Ordering::Relaxed);
    }
    if is_key_pressed(KeyCode::Space) {
        // Capture and save image
        let wf_data = state.wf_data.lock().unwrap();
        let path = format!("calibrations/{device_uuid}/images/widefield_{:02}.png", state.capture_count);
        PngEncoder::new(BufWriter::new(
            File::create(&path).unwrap(),
        ))
        .write_image(&wf_data.gray, u32::from(state.resolution.0), u32::from(state.resolution.1), ColorType::L8)
        .unwrap();
        let wf_data_gray = wf_data.gray.clone();
        drop(wf_data);
        state.add_capture(Port::Wf, wf_data_gray, path.clone());
        println!("Saved {path}");

        let nf_data = state.nf_data.lock().unwrap();
        let path = format!("calibrations/{device_uuid}/images/nearfield_{:02}.png", state.capture_count);
        PngEncoder::new(BufWriter::new(
            File::create(&path).unwrap(),
        ))
        .write_image(&nf_data.gray, u32::from(state.resolution.0), u32::from(state.resolution.1), ColorType::L8)
        .unwrap();
        let nf_data_gray = nf_data.gray.clone();
        drop(nf_data);
        state.add_capture(Port::Nf, nf_data_gray.clone(), path.clone());
        println!("Saved {path}");

        state.capture_count += 1;
    }
    if is_key_pressed(KeyCode::Backspace) {
        state.clear_captures();
    }
    if is_key_pressed(KeyCode::Key1) {
        println!(
            "Calibrating nearfield from {} captures",
            state.captured_nf.len()
        );
        let captures = state.captured_nf.clone();
        let captures_files = state.captured_nf_files.iter().map(|s| &**s).collect();
        let board_cols = state.detector_params.cols.load(Ordering::Relaxed);
        let board_rows = state.detector_params.rows.load(Ordering::Relaxed);
        let special = state.detector_params.special.load(Ordering::Relaxed);
        let pat = state.detector_params.pattern.load(Ordering::Relaxed);
        let resolution = state.resolution;
        let object_resolution = state.object_resolution;
        std::thread::spawn(move || match pat {
            DetectorPattern::AsymmetricCircles => circles::calibrate_single(
                &captures,
                &captures_files,
                Port::Nf,
                resolution,
                object_resolution,
                board_rows,
                board_cols,
                true,
                false,
                special,
                device_uuid,
            ),
            DetectorPattern::SymmetricCircles => circles::calibrate_single(
                &captures,
                &captures_files,
                Port::Nf,
                resolution,
                object_resolution,
                board_rows,
                board_cols,
                false,
                true,
                special,
                device_uuid,
            ),
            DetectorPattern::Chessboard => chessboard::calibrate_single(
                &captures,
                &captures_files,
                Port::Nf,
                resolution,
                object_resolution,
                board_rows,
                board_cols,
                device_uuid,
            ),
            DetectorPattern::None => eprintln!("No pattern selected"),
        });
    }
    if is_key_pressed(KeyCode::Key2) {
        println!(
            "Calibrating widefield from {} captures",
            state.captured_wf.len()
        );
        let captures = state.captured_wf.clone();
        let captures_files = state.captured_wf_files.iter().map(|s| &**s).collect();
        let board_cols = state.detector_params.cols.load(Ordering::Relaxed);
        let board_rows = state.detector_params.rows.load(Ordering::Relaxed);
        let special = state.detector_params.special.load(Ordering::Relaxed);
        let pat = state.detector_params.pattern.load(Ordering::Relaxed);
        let resolution = state.resolution;
        let object_resolution = state.object_resolution;
        std::thread::spawn(move || match pat {
            DetectorPattern::SymmetricCircles => circles::calibrate_single(
                &captures,
                &captures_files,
                Port::Wf,
                resolution,
                object_resolution,
                board_rows,
                board_cols,
                false,
                true,
                special,
                device_uuid,
            ),
            DetectorPattern::AsymmetricCircles => circles::calibrate_single(
                &captures,
                &captures_files,
                Port::Wf,
                resolution,
                object_resolution,
                board_rows,
                board_cols,
                true,
                false,
                special,
                device_uuid,
            ),
            DetectorPattern::Chessboard => chessboard::calibrate_single(
                &captures,
                &captures_files,
                Port::Wf,
                resolution,
                object_resolution,
                board_rows,
                board_cols,
                device_uuid,
            ),
            DetectorPattern::None => eprintln!("No pattern selected"),
        });
    }
    if is_key_pressed(KeyCode::Key3) {
        println!(
            "Calibrating stereo from {} captures",
            state.captured_wf.len()
        );
        let wf = state.captured_wf.clone();
        let nf = state.captured_nf.clone();
        let captures_nf_files = state.captured_nf_files.iter().map(|s| &**s).collect();
        let captures_wf_files = state.captured_wf_files.iter().map(|s| &**s).collect();
        let board_cols = state.detector_params.cols.load(Ordering::Relaxed);
        let board_rows = state.detector_params.rows.load(Ordering::Relaxed);
        let special = state.detector_params.special.load(Ordering::Relaxed);
        let pat = state.detector_params.pattern.load(Ordering::Relaxed);
        let resolution = state.resolution;
        let object_resolution = state.object_resolution;
        std::thread::spawn(move || match pat {
            DetectorPattern::Chessboard => chessboard::my_stereo_calibrate(
                &wf,
                &nf,
                resolution,
                object_resolution,
                board_rows,
                board_cols,
                &captures_wf_files,
                &captures_nf_files,
                device_uuid,
            ),
            DetectorPattern::AsymmetricCircles => circles::my_stereo_calibrate(
                &wf,
                &nf,
                resolution,
                object_resolution,
                board_rows,
                board_cols,
                true,
                false,
                special,
                &captures_wf_files,
                &captures_nf_files,
                device_uuid,
            ),
            DetectorPattern::SymmetricCircles => circles::my_stereo_calibrate(
                &wf,
                &nf,
                resolution,
                object_resolution,
                board_rows,
                board_cols,
                false,
                true,
                special,
                &captures_wf_files,
                &captures_nf_files,
                device_uuid,
            ),
            DetectorPattern::None => eprintln!("No pattern selected"),
        });
    }
    if is_key_pressed(KeyCode::R) {
        if is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift) {
            state.detector_params.rows.fetch_sub(1, Ordering::Relaxed);
        } else {
            state.detector_params.rows.fetch_add(1, Ordering::Relaxed);
        }
    }
    if is_key_pressed(KeyCode::C) {
        if is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift) {
            state.detector_params.cols.fetch_sub(1, Ordering::Relaxed);
        } else {
            state.detector_params.cols.fetch_add(1, Ordering::Relaxed);
        }
    }
    if is_key_pressed(KeyCode::S) {
        state.detector_params.special.store(!state.detector_params.special.load(Ordering::Relaxed), Ordering::Relaxed);
    }
    if is_key_pressed(KeyCode::D) {
        use DetectorPattern as P;
        let pattern = state.detector_params.pattern.load(Ordering::Relaxed);
        let new_value = if is_key_down(KeyCode::LeftShift) || is_key_down(KeyCode::RightShift) {
            match pattern {
                P::None => P::AsymmetricCircles,
                P::AsymmetricCircles => P::SymmetricCircles,
                P::SymmetricCircles => P::Chessboard,
                P::Chessboard => P::None,
            }
        } else {
            match pattern {
                P::None => P::Chessboard,
                P::Chessboard => P::SymmetricCircles,
                P::SymmetricCircles => P::AsymmetricCircles,
                P::AsymmetricCircles => P::None,
            }
        };
        state.detector_params.pattern.store(new_value, Ordering::Relaxed);
    }
    if is_key_pressed(KeyCode::F1) {
        state.reset.store(true, Ordering::Relaxed);
    }
}

fn paj_reader_thread(
    mut port: Box<dyn SerialPort>,
    object_resolution: (u32, u32),
    wf_data: Arc<Mutex<PajData>>,
    nf_data: Arc<Mutex<PajData>>,
    detector_params: Arc<DetectorParams>,
    reset: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
) {

    let mut buf = [0; 9898];
    loop {
        if reset.load(Ordering::Relaxed) {
            port.write(b"r").unwrap();
            port.flush().unwrap();
            quit.store(true, Ordering::Relaxed);
            break;
        } else {
            port.write(b"a").unwrap();
        }
        port.read_exact(&mut buf).unwrap();
        let [.., id, _, _] = buf;

        let board_cols = detector_params.cols.load(Ordering::Relaxed);
        let board_rows = detector_params.rows.load(Ordering::Relaxed);
        let special = detector_params.special.load(Ordering::Relaxed);
        let pattern = detector_params.pattern.load(Ordering::Relaxed);
        let port = match id {
            0 => Port::Wf,
            _ => Port::Nf,
        };
        let gray: &[u8; 98 * 98] = &buf[..98 * 98].try_into().unwrap();
        let pattern_points = match pattern {
            DetectorPattern::None => None,
            DetectorPattern::Chessboard => {
                chessboard::get_chessboard_corners(
                    gray,
                    port,
                    (98, 98),
                    object_resolution,
                    board_rows,
                    board_cols,
                    false,
                )
            }
            DetectorPattern::AsymmetricCircles => {
                circles::get_circles_centers(
                    gray,
                    port,
                    (98, 98),
                    object_resolution,
                    board_rows,
                    board_cols,
                    false,
                    true,
                    false,
                    special,
                )
            }
            DetectorPattern::SymmetricCircles => {
                circles::get_circles_centers(
                    gray,
                    port,
                    (98, 98),
                    object_resolution,
                    board_rows,
                    board_cols,
                    false,
                    false,
                    true,
                    special,
                )
            }
        };

        let mut paj_data = if id == 0 {
            wf_data.lock().unwrap()
        } else {
            nf_data.lock().unwrap()
        };
        paj_data.image.resize(4*98*98, 255);
        for i in 0..98 * 98 {
            paj_data.image[4 * i] = buf[i];
            paj_data.image[4 * i + 1] = buf[i];
            paj_data.image[4 * i + 2] = buf[i];
        }
        paj_data.gray.resize(gray.len(), 0);
        paj_data.gray.copy_from_slice(gray);
        for (i, mut raw_mot) in buf[98 * 98..][..256].chunks(16).enumerate() {
            paj_data.mot_data[i] = MotData::parse(&mut raw_mot);
        }
        paj_data.avg_frame_period = paj_data.avg_frame_period * 7. / 8.
            + paj_data.last_frame.elapsed().as_secs_f64() / 8.;
        paj_data.last_frame = Instant::now();
        paj_data.pattern_points = pattern_points.map(|v| v.into());
    }
}

fn image_only_reader_thread(
    mut port: Box<dyn SerialPort>,
    resolution: (u16, u16),
    object_resolution: (u32, u32),
    wf_data: Arc<Mutex<PajData>>,
    _nf_data: Arc<Mutex<PajData>>,
    detector_params: Arc<DetectorParams>,
    reset: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
) {

    let mut gray = [0; 320*240];
    loop {
        if reset.load(Ordering::Relaxed) {
            port.write(b"r").unwrap();
            port.flush().unwrap();
            quit.store(true, Ordering::Relaxed);
            break;
        } else {
            port.write(b"a").unwrap();
        }
        port.read_exact(&mut gray).unwrap();

        let board_cols = detector_params.cols.load(Ordering::Relaxed);
        let board_rows = detector_params.rows.load(Ordering::Relaxed);
        let special = detector_params.special.load(Ordering::Relaxed);
        let pattern = detector_params.pattern.load(Ordering::Relaxed);
        let port = Port::Wf;
        let pattern_points = match pattern {
            DetectorPattern::None => None,
            DetectorPattern::Chessboard => {
                chessboard::get_chessboard_corners(
                    &gray,
                    port,
                    resolution,
                    object_resolution,
                    board_rows,
                    board_cols,
                    false,
                )
            }
            DetectorPattern::AsymmetricCircles => {
                circles::get_circles_centers(
                    &gray,
                    port,
                    resolution,
                    object_resolution,
                    board_rows,
                    board_cols,
                    false,
                    true,
                    false,
                    special,
                )
            }
            DetectorPattern::SymmetricCircles => {
                circles::get_circles_centers(
                    &gray,
                    port,
                    resolution,
                    object_resolution,
                    board_rows,
                    board_cols,
                    false,
                    false,
                    true,
                    special,
                )
            }
        };

        let mut wf_data = wf_data.lock().unwrap();
        wf_data.image.resize(4*320*240, 255);
        for (i, &v) in gray.iter().enumerate() {
            wf_data.image[4 * i] = v;
            wf_data.image[4 * i + 1] = v;
            wf_data.image[4 * i + 2] = v;
        }
        wf_data.gray.resize(gray.len(), 0);
        wf_data.gray.copy_from_slice(&gray);
        wf_data.avg_frame_period = wf_data.avg_frame_period * 7. / 8.
            + wf_data.last_frame.elapsed().as_secs_f64() / 8.;
        wf_data.last_frame = Instant::now();
        wf_data.pattern_points = pattern_points.map(|v| v.into());
    }
}

fn _opencv_thread(
    raw_image: Receiver<(Port, Vec<u8>)>,
    detector_params: Arc<DetectorParams>,
    resolution: (u16, u16),
    object_resolution: (u32, u32),
) {
    loop {
        let (port, image) = raw_image.recv().unwrap();
        let board_cols = detector_params.cols.load(Ordering::Relaxed);
        let board_rows = detector_params.rows.load(Ordering::Relaxed);
        let special = detector_params.special.load(Ordering::Relaxed);
        let pattern = detector_params.pattern.load(Ordering::Relaxed);
        match pattern {
            DetectorPattern::None => (),
            DetectorPattern::Chessboard => {
                chessboard::get_chessboard_corners(
                    &image,
                    Port::Wf,
                    resolution,
                    object_resolution,
                    board_rows,
                    board_cols,
                    true,
                );
            }
            DetectorPattern::AsymmetricCircles => {
                circles::get_circles_centers(
                    &image,
                    port,
                    resolution,
                    object_resolution,
                    board_rows,
                    board_cols,
                    true,
                    true,
                    false,
                    special,
                );
            }
            DetectorPattern::SymmetricCircles => {
                circles::get_circles_centers(
                    &image,
                    port,
                    resolution,
                    object_resolution,
                    board_rows,
                    board_cols,
                    true,
                    false,
                    true,
                    special,
                );
            }
        }
    }
}

fn drain<T: SerialPort + ?Sized>(port: &mut T) -> u32 {
    let mut drained = 0;
    loop {
        let bytes_to_read = port.bytes_to_read().unwrap();
        if bytes_to_read > 0 {
            port.clear(ClearBuffer::Input).unwrap();
            drained += bytes_to_read;
            std::thread::sleep(Duration::from_millis(7));
        } else {
            return drained;
        }
    }
}

fn save_single_calibration(
    port: Port,
    pattern: &str,
    camera_matrix: &Mat,
    dist_coeffs: &Mat,
    reproj_err: f64,
    image_files: &Vector<String>,
    device_uuid: DeviceUuid,
) {
    let filename = match port {
        Port::Nf => format!("calibrations/{device_uuid}/nearfield.json"),
        Port::Wf => format!("calibrations/{device_uuid}/widefield.json"),
    };
    let mut fs =
        FileStorage::new_def(&filename, FileStorage_WRITE | FileStorage_FORMAT_JSON).unwrap();
    if fs.is_opened().unwrap() {
        fs.write_mat("camera_matrix", camera_matrix).unwrap();
        fs.write_mat("dist_coeffs", dist_coeffs).unwrap();
        fs.write_f64("rms_error", reproj_err).unwrap();
        fs.write_str("method", &pattern.to_string()).unwrap();
        fs.write_i32("version", CALIBRATION_VERSION).unwrap();
        fs.write_str_vec("image_files", image_files).unwrap();
        fs.release().unwrap();
    } else {
        println!("Failed to open {}", filename);
    }
}

fn save_stereo_calibration(
    pattern: &str,
    r: Mat,
    t: Mat,
    reproj_err: f64,
    wf_image_files: &Vector<String>,
    nf_image_files: &Vector<String>,
    device_uuid: DeviceUuid,
) {
    let mut fs =
        FileStorage::new_def(&format!("calibrations/{device_uuid}/stereo.json"), FileStorage_WRITE | FileStorage_FORMAT_JSON).unwrap();
    if fs.is_opened().unwrap() {
        fs.write_mat("r", &r).unwrap();
        fs.write_mat("t", &t).unwrap();
        fs.write_f64("rms_error", reproj_err).unwrap();
        fs.write_i32("num_captures", wf_image_files.len() as i32).unwrap();
        fs.write_str("method", &pattern.to_string()).unwrap();
        fs.write_i32("version", CALIBRATION_VERSION).unwrap();
        fs.write_str_vec("wf_image_files", wf_image_files).unwrap();
        fs.write_str_vec("nf_image_files", nf_image_files).unwrap();
    } else {
        println!("Failed to open stereo.json");
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct DeviceUuid([u8; 6]);

impl Display for DeviceUuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f,
            "{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
            self.0[0],
            self.0[1],
            self.0[2],
            self.0[3],
            self.0[4],
            self.0[5],
        )
    }
}
