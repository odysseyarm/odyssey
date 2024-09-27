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
use opencv::core::Point2f;
use serialport::{ClearBuffer, SerialPort};

pub mod mot_data;
pub mod charuco;
pub mod chessboard;
pub mod circles;
pub mod draw_pattern_points;

use macroquad::prelude::*;

const CALIBRATION_VERSION: i32 = 1;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Port {
    Wf,
    Nf,
}

struct PajData {
    image: [u8; 98 * 98 * 4],
    gray: [u8; 98 * 98],
    mot_data: [MotData; 16],
    pattern_points: Option<Vec<Point2f>>,
    avg_frame_period: f64,
    last_frame: Instant,
}

impl Default for PajData {
    fn default() -> Self {
        Self {
            image: [255; 98 * 98 * 4],
            gray: [255; 98 * 98],
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
    captured_nf: Vec<[u8; 98 * 98]>,
    captured_wf: Vec<[u8; 98 * 98]>,
    detector_params: Arc<DetectorParams>,
    reset: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
    draw_pattern_points: DrawPatternPoints,
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
    fn new(capture_count: usize) -> MainState {
        let wf_data = Arc::new(Mutex::new(Default::default()));
        let nf_data = Arc::new(Mutex::new(Default::default()));
        let reset = Arc::new(AtomicBool::new(false));
        let quit = Arc::new(AtomicBool::new(false));
        let path = std::env::args().nth(1).unwrap_or("/dev/ttyACM1".into());
        let binding = std::env::args().nth(2).unwrap_or("".into());

        let main_state = MainState {
            wf_data: wf_data.clone(),
            nf_data: nf_data.clone(),
            capture_count,
            captured_nf: vec![],
            captured_wf: vec![],
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
        std::thread::spawn(move || {
            reader_thread(
                path,
                wf_data,
                nf_data,
                detector_params,
                reset,
                quit,
            );
        });

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
        let image_scale = 5.0;
        let image_size = 98.0 * image_scale;
        let spacing = 20.0;

        // Draw images
        draw_image(
            &wf_data.image,
            vec2(0.0, 0.0),
            image_scale,
            false,  // Flip vertically
            true,  // Flip horizontally
        );
        draw_image(
            &nf_data.image,
            vec2(image_size + spacing, 0.0),
            image_scale,
            true,  // Flip vertically
            false,  // Flip horizontally
        );

        // Draw FPS
        let fps = 1.0 / wf_data.avg_frame_period;
        draw_text(&format!("{:.1} fps", fps), 10.0, image_size + 20.0, 20.0, WHITE);
        draw_text(
            &format!("{:.1} fps", fps),
            image_size + spacing + 10.0,
            image_size + 20.0,
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
            image_size + 40.0,
            20.0,
            WHITE,
        );
        draw_text(
            "<1> calib nf    <2> calib wf    <3> stereo calib    <Backspace> clear    <Esc> close",
            10.0,
            image_size + 60.0,
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
            vec2(image_size + spacing, 0.0),
            vec2(0., 0.),
        );

        // Draw circles
        draw_mot_data_circles(&wf_data.mot_data, wf_transform, false, true);
        draw_mot_data_circles(&nf_data.mot_data, nf_transform, true, false);

        // Draw pattern points if available
        if let Some(pattern_points) = &wf_data.pattern_points {
            self.draw_pattern_points.draw(
                pattern_points,
                cols.into(),
                true,
                wf_transform,
            );
        }
        if let Some(pattern_points) = &nf_data.pattern_points {
            self.draw_pattern_points.draw(
                pattern_points,
                cols.into(),
                true,
                nf_transform,
            );
        }
    }
}

fn draw_image(
    image: &[u8; 98 * 98 * 4],
    offset: Vec2,
    scale: f32,
    flip_y: bool,
    flip_x: bool,
) {
    for (i, pixel) in image.chunks(4).enumerate() {
        let ix = (i % 98) as f32;
        let iy = (i / 98) as f32;
        let mut dest_x = ix * scale;
        let mut dest_y = iy * scale;
        if flip_x {
            dest_x = 98.0 * scale - dest_x - scale; // Adjust for scale
        }
        if flip_y {
            dest_y = 98.0 * scale - dest_y - scale; // Adjust for scale
        }
        draw_rectangle(
            offset.x + dest_x,
            offset.y + dest_y,
            scale,
            scale,
            Color::from_rgba(pixel[0], pixel[1], pixel[2], pixel[3]),
        );
    }
}

fn draw_mot_data_circles(
    mot_data: &[MotData],
    transform: Mat4,
    flip_y: bool,
    flip_x: bool,
) {
    for data in mot_data {
        if data.area > 0 {
            let mut point = vec3(
                data.cx as f32 / 4094. * 97. + 0.5,
                data.cy as f32 / 4094. * 97. + 0.5,
                0.0,
            );
            if flip_x {
                point.x = 98.0 - point.x;
            }
            if flip_y {
                point.y = 98.0 - point.y;
            }
            let transformed_point = transform.transform_point3(point);
            draw_circle_lines(transformed_point.x, transformed_point.y, 4.0, 1.0, RED);
        }
    }
}

pub async fn main() {
    // Manually set window dimensions
    request_new_screen_size(98.0 * 10.0 + 40.0, 98.0 * 5.0 + 80.0);

    std::fs::create_dir_all("images").unwrap();
    let capture_count = (0..)
        .find(|i| !Path::new(&format!("images/nearfield_{:02}.png", i)).exists())
        .unwrap();
    let mut state = MainState::new(capture_count);

    loop {
        state.update();
        state.draw();
        handle_input(&mut state).await;
        next_frame().await;
    }
}

async fn handle_input(state: &mut MainState) {
    if is_key_pressed(KeyCode::Escape) {
        state.quit.store(true, Ordering::Relaxed);
    }
    if is_key_pressed(KeyCode::Space) {
        // Capture and save image
        let wf_data = state.wf_data.lock().unwrap();
        PngEncoder::new(BufWriter::new(
            File::create(format!("images/widefield_{:02}.png", state.capture_count)).unwrap(),
        ))
        .write_image(&wf_data.gray, 98, 98, ColorType::L8)
        .unwrap();
        state.captured_wf.push(wf_data.gray);
        drop(wf_data);
        println!("Saved images/widefield_{:02}.png", state.capture_count);

        let nf_data = state.nf_data.lock().unwrap();
        PngEncoder::new(BufWriter::new(
            File::create(format!("images/nearfield_{:02}.png", state.capture_count)).unwrap(),
        ))
        .write_image(&nf_data.gray, 98, 98, ColorType::L8)
        .unwrap();
        state.captured_nf.push(nf_data.gray);
        drop(nf_data);
        println!("Saved images/nearfield_{:02}.png", state.capture_count);

        state.capture_count += 1;
    }
    if is_key_pressed(KeyCode::Backspace) {
        state.captured_nf.clear();
        state.captured_wf.clear();
    }
    if is_key_pressed(KeyCode::Key1) {
        println!(
            "Calibrating nearfield from {} captures",
            state.captured_nf.len()
        );
        let captures = state.captured_nf.clone();
        let board_cols = state.detector_params.cols.load(Ordering::Relaxed);
        let board_rows = state.detector_params.rows.load(Ordering::Relaxed);
        let special = state.detector_params.special.load(Ordering::Relaxed);
        let pat = state.detector_params.pattern.load(Ordering::Relaxed);
        std::thread::spawn(move || match pat {
            DetectorPattern::AsymmetricCircles => circles::calibrate_single(
                &captures,
                Port::Nf,
                board_rows,
                board_cols,
                true,
                false,
                special,
            ),
            DetectorPattern::SymmetricCircles => circles::calibrate_single(
                &captures,
                Port::Nf,
                board_rows,
                board_cols,
                false,
                true,
                special,
            ),
            DetectorPattern::Chessboard => chessboard::calibrate_single(
                &captures,
                Port::Nf,
                board_rows,
                board_cols,
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
        let board_cols = state.detector_params.cols.load(Ordering::Relaxed);
        let board_rows = state.detector_params.rows.load(Ordering::Relaxed);
        let special = state.detector_params.special.load(Ordering::Relaxed);
        let pat = state.detector_params.pattern.load(Ordering::Relaxed);
        std::thread::spawn(move || match pat {
            DetectorPattern::SymmetricCircles => circles::calibrate_single(
                &captures,
                Port::Wf,
                board_rows,
                board_cols,
                false,
                true,
                special,
            ),
            DetectorPattern::AsymmetricCircles => circles::calibrate_single(
                &captures,
                Port::Wf,
                board_rows,
                board_cols,
                true,
                false,
                special,
            ),
            DetectorPattern::Chessboard => chessboard::calibrate_single(
                &captures,
                Port::Wf,
                board_rows,
                board_cols,
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
        let board_cols = state.detector_params.cols.load(Ordering::Relaxed);
        let board_rows = state.detector_params.rows.load(Ordering::Relaxed);
        let special = state.detector_params.special.load(Ordering::Relaxed);
        let pat = state.detector_params.pattern.load(Ordering::Relaxed);
        std::thread::spawn(move || match pat {
            DetectorPattern::Chessboard => chessboard::my_stereo_calibrate(
                &wf,
                &nf,
                board_rows,
                board_cols,
            ),
            DetectorPattern::AsymmetricCircles => circles::my_stereo_calibrate(
                &wf,
                &nf,
                board_rows,
                board_cols,
                true,
                false,
                special,
            ),
            DetectorPattern::SymmetricCircles => circles::my_stereo_calibrate(
                &wf,
                &nf,
                board_rows,
                board_cols,
                false,
                true,
                special,
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

fn reader_thread(
    path: String,
    wf_data: Arc<Mutex<PajData>>,
    nf_data: Arc<Mutex<PajData>>,
    detector_params: Arc<DetectorParams>,
    reset: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
) {
    let mut port = serialport::new(path, 115200)
        .timeout(Duration::from_secs(3))
        .open()
        .unwrap();
    let drained = drain(&mut *port);
    if drained > 0 {
        eprintln!("drained {drained} bytes");
    }

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
        let [.., id, len0, len1] = buf;
        let len = u16::from_le_bytes([len0, len1]);
        if len == 9898 {
            shiftl(&mut buf, 4);
        } else {
            if len < 9897 {
                buf.rotate_right(9897 - len as usize);
                buf[..9897 - len as usize].fill(0);
            }
            for (i, &byte) in [0x2d, 0x5a, 0xb4, 0x69, 0xd2, 0xa5, 0x4b].iter().enumerate() {
                if buf[buf.len() - 6] == byte {
                    shiftr(&mut buf, i as u8 + 1);
                }
            }
        }

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
                    &gray,
                    port,
                    board_rows,
                    board_cols,
                    false,
                )
            }
            DetectorPattern::AsymmetricCircles => {
                circles::get_circles_centers(
                    &gray,
                    port,
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
        for i in 0..98 * 98 {
            paj_data.image[4 * i] = buf[i];
            paj_data.image[4 * i + 1] = buf[i];
            paj_data.image[4 * i + 2] = buf[i];
        }
        paj_data.gray = *gray;
        for (i, mut raw_mot) in buf[98 * 98..][..256].chunks(16).enumerate() {
            paj_data.mot_data[i] = MotData::parse(&mut raw_mot);
        }
        paj_data.avg_frame_period = paj_data.avg_frame_period * 7. / 8.
            + paj_data.last_frame.elapsed().as_secs_f64() / 8.;
        paj_data.last_frame = Instant::now();
        paj_data.pattern_points = pattern_points.map(|v| v.into());
    }
}

fn opencv_thread(
    raw_image: Receiver<(Port, [u8; 98 * 98])>,
    detector_params: Arc<DetectorParams>,
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
                    board_rows,
                    board_cols,
                    true,
                );
            }
            DetectorPattern::AsymmetricCircles => {
                circles::get_circles_centers(
                    &image,
                    port,
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

/// Shift the data to the left n bits
fn shiftl(d: &mut [u8], n: u8) {
    d[0] <<= n;
    for i in 1..d.len() {
        d[i - 1] |= d[i] >> (8 - n);
        d[i] <<= n;
    }
}

/// Shift the data to the right n bits
fn shiftr(d: &mut [u8], n: u8) {
    let mut t = 0;
    for i in 0..d.len() {
        let t2 = d[i];
        d[i] >>= n;
        d[i] |= t << (8 - n);
        t = t2;
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
