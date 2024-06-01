use std::{
    fmt::Display, fs::File, io::BufWriter, path::Path, sync::{atomic::{AtomicBool, AtomicU16, Ordering}, mpsc::{Receiver, SyncSender}, Arc, Mutex}, time::{Duration, Instant}
};

use draw_pattern_points::DrawPatternPoints;
use ggez::{
    conf::{WindowMode, WindowSetup}, event, glam::*, graphics::{self, Color, DrawParam, ImageFormat, Sampler, Text, Transform}, input::{keyboard::{KeyCode, KeyMods}, mouse::{cursor_grabbed, set_cursor_grabbed, set_position}}, Context, GameResult
};
use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
use mot_data::MotData;
use opencv::core::{Point2f, Vector};
use serialport::{ClearBuffer, SerialPort};

pub mod mot_data;
pub mod charuco;
pub mod chessboard;
pub mod circles;
pub mod draw_pattern_points;

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
            image: [255; 98*98*4],
            gray: [255; 98*98],
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
    circle: ggez::graphics::Mesh,
    draw_pattern_points: DrawPatternPoints,
    capture_count: usize,
    captured_nf: Vec<[u8; 98 * 98]>,
    captured_wf: Vec<[u8; 98 * 98]>,
    detector_params: Arc<DetectorParams>,
    upside_down: bool,
    reset: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
}

#[derive(Debug)]
struct DetectorParams {
    rows: AtomicU16,
    cols: AtomicU16,
    pattern: atomic::Atomic<DetectorPattern>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, bytemuck::NoUninit)]
#[repr(u8)]
enum DetectorPattern {
    None,
    Chessboard,
    Acircles,
}

impl Display for DetectorPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DetectorPattern::None => write!(f, "off"),
            DetectorPattern::Chessboard => write!(f, "chessboard"),
            DetectorPattern::Acircles => write!(f, "acircles"),
        }
    }
}

impl MainState {
    fn new(ctx: &mut Context, capture_count: usize) -> GameResult<MainState> {
        let circle = graphics::Mesh::new_circle(
            ctx,
            graphics::DrawMode::fill(),
            vec2(0., 0.),
            4.0,
            2.0,
            Color::RED,
        )?;

        let wf_data = Arc::new(Mutex::new(Default::default()));
        let nf_data = Arc::new(Mutex::new(Default::default()));
        // let img_channel = std::sync::mpsc::sync_channel(2);
        let reset = Arc::new(AtomicBool::new(false));
        let quit = Arc::new(AtomicBool::new(false));
        let path = std::env::args().nth(1).unwrap_or("/dev/ttyACM1".into());
        let binding = std::env::args().nth(2).unwrap_or("".into());
        let upside_down = binding.as_str();
        let upside_down = match upside_down {
            "--upside-down" | "-ud" => true,
            _ => false,
        };
        println!("upside_down: {}", upside_down);
        let main_state = MainState {
            wf_data: wf_data.clone(),
            nf_data: nf_data.clone(),
            circle,
            draw_pattern_points: DrawPatternPoints::new(ctx),
            capture_count,
            captured_nf: vec![],
            captured_wf: vec![],
            detector_params: DetectorParams {
                rows: 6.into(),
                cols: 6.into(),
                pattern: atomic::Atomic::new(DetectorPattern::Chessboard),
            }.into(),
            reset: reset.clone(),
            quit: quit.clone(),
            upside_down,
        };
        let detector_params = main_state.detector_params.clone();
        std::thread::spawn(move || {
            reader_thread(path, wf_data, nf_data, detector_params, upside_down, reset, quit);
        });
        // let detector_params = main_state.detector_params.clone();
        // std::thread::spawn(move || opencv_thread(img_channel.1, detector_params, upside_down));
        Ok(main_state)
    }
}

impl event::EventHandler<ggez::GameError> for MainState {
    fn update(&mut self, ctx: &mut Context) -> GameResult {
        // self.pos_x = self.pos_x % 800.0 + 1.0;
        if self.quit.load(Ordering::Relaxed) {
            ctx.request_quit();
        }
        Ok(())
    }

    fn draw(&mut self, ctx: &mut Context) -> GameResult {
        let mut canvas =
            graphics::Canvas::from_frame(ctx, graphics::Color::from([0.1, 0.2, 0.3, 1.0]));
        canvas.set_sampler(Sampler::nearest_clamp());

        let wf_data = self.wf_data.lock().unwrap();
        let nf_data = self.nf_data.lock().unwrap();
        let wf_image = graphics::Image::from_pixels(
            ctx,
            &wf_data.image[..98 * 98 * 4],
            ImageFormat::Rgba8UnormSrgb,
            98,
            98,
        );
        let nf_image = graphics::Image::from_pixels(
            ctx,
            &nf_data.image[..98 * 98 * 4],
            ImageFormat::Rgba8UnormSrgb,
            98,
            98,
        );

        canvas.draw(
            &wf_image,
            DrawParam {
                transform: Transform::Values {
                    dest: [7.*98., 0.].into(),
                    rotation: 0.,
                    scale: [-7., 7.].into(),
                    offset: [0., 0.].into(),
                },
                ..Default::default()
            },
        );
        canvas.draw(
            &nf_image,
            DrawParam {
                transform: Transform::Values {
                    dest: [700., 7.*98.].into(),
                    rotation: 0.,
                    scale: [7., -7.].into(),
                    offset: [0., 0.].into(),
                },
                ..Default::default()
            },
        );
        canvas.draw(&Text::new(format!("{:.1} fps", 1. / nf_data.avg_frame_period)), DrawParam {
            color: Color::WHITE,
            transform: Transform::Values {
                dest: [700., 7.*98.].into(),
                rotation: 0.,
                scale: [1., 1.].into(),
                offset: [0., 0.].into(),
            },
            ..Default::default()
        });
        canvas.draw(&Text::new(format!("{:.1} fps", 1. / nf_data.avg_frame_period)), DrawParam {
            color: Color::WHITE,
            transform: Transform::Values {
                dest: [0., 7.*98.].into(),
                rotation: 0.,
                scale: [1., 1.].into(),
                offset: [0., 0.].into(),
            },
            ..Default::default()
        });
        let cols = self.detector_params.cols.load(Ordering::Relaxed);
        let rows = self.detector_params.rows.load(Ordering::Relaxed);
        let pat = self.detector_params.pattern.load(Ordering::Relaxed);
        canvas.draw(&Text::new(format!("Captures (space): {}    Board (r, c): {rows}x{cols}    Detector (d): {pat}", self.captured_wf.len())), DrawParam {
            color: Color::WHITE,
            transform: Transform::Values {
                dest: [0., 7.*98. + 18.].into(),
                rotation: 0.,
                scale: [1., 1.].into(),
                offset: [0., 0.].into(),
            },
            ..Default::default()
        });
        canvas.draw(&Text::new("<1> calib nf    <2> calib wf    <3> stereo calib    <Backspace> clear    <Esc> close"), DrawParam {
            color: Color::WHITE,
            transform: Transform::Values {
                dest: [0., 7.*98. + 36.].into(),
                rotation: 0.,
                scale: [1., 1.].into(),
                offset: [0., 0.].into(),
            },
            ..Default::default()
        });

        for mot_data in &wf_data.mot_data {
            if mot_data.area > 0 {
                let x = mot_data.cx as f32 / 2940.;
                let y = mot_data.cy as f32 / 2940.;
                canvas.draw(&self.circle, Vec2::new((1.-x)*7.*98., y*7.*98.));
            }
        }

        for mot_data in &nf_data.mot_data {
            if mot_data.area > 0 {
                let x = mot_data.cx as f32 / 2940.;
                let y = mot_data.cy as f32 / 2940.;
                canvas.draw(&self.circle, Vec2::new(x*7.*98.+700., (1.-y)*7.*98.));
            }
        }

        if let Some(pattern_points) = &wf_data.pattern_points {
            self.draw_pattern_points.draw(ctx, &mut canvas, pattern_points, cols.into(), true, Transform::Values {
                dest: [0., 0.].into(),
                rotation: 0.,
                scale: [7., 7.].into(),
                offset: [0., 0.].into(),
            });
        }

        if let Some(pattern_points) = &nf_data.pattern_points {
            self.draw_pattern_points.draw(ctx, &mut canvas, pattern_points, cols.into(), true, Transform::Values {
                dest: [700., 0.].into(),
                rotation: 0.,
                scale: [7., 7.].into(),
                offset: [0., 0.].into(),
            });
        }

        canvas.finish(ctx)?;

        Ok(())
    }

    fn key_down_event(
        &mut self,
        ctx: &mut Context,
        input: ggez::input::keyboard::KeyInput,
        repeated: bool,
    ) -> Result<(), ggez::GameError> {
        if repeated {
            return Ok(());
        }
        match input.keycode {
            Some(KeyCode::Escape) => ctx.request_quit(),
            Some(KeyCode::Space) => {
                // capture and save image
                let wf_data = self.wf_data.lock().unwrap();
                PngEncoder::new(BufWriter::new(File::create(format!("images/widefield_{:02}.png", self.capture_count)).unwrap())).write_image(&wf_data.gray, 98, 98, ColorType::L8).unwrap();
                self.captured_wf.push(wf_data.gray);
                drop(wf_data);
                println!("Saved images/widefield_{:02}.png", self.capture_count);
                let nf_data = self.nf_data.lock().unwrap();
                PngEncoder::new(BufWriter::new(File::create(format!("images/nearfield_{:02}.png", self.capture_count)).unwrap())).write_image(&nf_data.gray, 98, 98, ColorType::L8).unwrap();
                self.captured_nf.push(nf_data.gray);
                println!("Saved images/nearfield_{:02}.png", self.capture_count);
                self.capture_count += 1;
            }
            Some(KeyCode::Back) => {
                self.captured_nf.clear();
                self.captured_wf.clear();
            }
            Some(KeyCode::Key1) => {
                // calibrate nf
                println!("Calibrating nearfield from {} captures", self.captured_nf.len());
                let captures = self.captured_nf.clone();
                let board_cols = self.detector_params.cols.load(Ordering::Relaxed);
                let board_rows = self.detector_params.rows.load(Ordering::Relaxed);
                let pat = self.detector_params.pattern.load(Ordering::Relaxed);
                std::thread::spawn({
                    let upside_down = self.upside_down;
                    move || {
                        match pat {
                            DetectorPattern::Acircles => circles::calibrate_single(&captures, Port::Nf, board_rows, board_cols, upside_down),
                            DetectorPattern::Chessboard => chessboard::calibrate_single(&captures, Port::Nf, board_rows, board_cols, upside_down),
                            DetectorPattern::None => eprintln!("No pattern selected"),
                        }
                    }
                });
            }
            Some(KeyCode::Key2) => {
                // calibrate wf
                println!("Calibrating widefield from {} captures", self.captured_wf.len());
                let captures = self.captured_wf.clone();
                let board_cols = self.detector_params.cols.load(Ordering::Relaxed);
                let board_rows = self.detector_params.rows.load(Ordering::Relaxed);
                let pat = self.detector_params.pattern.load(Ordering::Relaxed);
                std::thread::spawn({
                    let upside_down = self.upside_down;
                    move || {
                        match pat {
                            DetectorPattern::Acircles => circles::calibrate_single(&captures, Port::Wf, board_rows, board_cols, upside_down),
                            DetectorPattern::Chessboard => chessboard::calibrate_single(&captures, Port::Wf, board_rows, board_cols, upside_down),
                            DetectorPattern::None => eprintln!("No pattern selected"),
                        }
                    }
                });
            }
            Some(KeyCode::Key3) => {
                println!("Calibrating stereo from {} captures", self.captured_wf.len());
                let wf = self.captured_wf.clone();
                let nf = self.captured_nf.clone();
                let board_cols = self.detector_params.cols.load(Ordering::Relaxed);
                let board_rows = self.detector_params.rows.load(Ordering::Relaxed);
                let pat = self.detector_params.pattern.load(Ordering::Relaxed);
                std::thread::spawn({
                    let upside_down = self.upside_down;
                    move || {
                        match pat {
                            DetectorPattern::Chessboard => chessboard::my_stereo_calibrate(&wf, &nf, board_rows, board_cols, upside_down),
                            DetectorPattern::Acircles => eprintln!("Not implemented!"),
                            DetectorPattern::None => eprintln!("No pattern selected"),
                        }
                    }
                });
            }
            Some(KeyCode::R) => {
                if input.mods.contains(KeyMods::SHIFT) {
                    self.detector_params.rows.fetch_sub(1, Ordering::Relaxed);
                } else {
                    self.detector_params.rows.fetch_add(1, Ordering::Relaxed);
                }
            }
            Some(KeyCode::C) => {
                if input.mods.contains(KeyMods::SHIFT) {
                    self.detector_params.cols.fetch_sub(1, Ordering::Relaxed);
                } else {
                    self.detector_params.cols.fetch_add(1, Ordering::Relaxed);
                }
            }
            Some(KeyCode::D) => {
                use DetectorPattern as P;
                let pattern = self.detector_params.pattern.load(Ordering::Relaxed);
                let new_value = if input.mods.contains(KeyMods::SHIFT) {
                    match pattern {
                        P::None => P::Acircles,
                        P::Chessboard => P::None,
                        P::Acircles => P::Chessboard,
                    }
                } else {
                    match pattern {
                        P::None => P::Chessboard,
                        P::Chessboard => P::Acircles,
                        P::Acircles => P::None,
                    }
                };
                self.detector_params.pattern.store(new_value, Ordering::Relaxed);
            }
            Some(KeyCode::F1) => {
                self.reset.store(true, Ordering::Relaxed);
            }
            _ => (),
        }
        Ok(())
    }

    // fn mouse_button_down_event(
    //         &mut self,
    //         ctx: &mut Context,
    //         button: event::MouseButton,
    //         _x: f32,
    //         _y: f32,
    //     ) -> Result<(), ggez::GameError> {
    //     match button {
    //         event::MouseButton::Left => set_cursor_grabbed(ctx, true),
    //         _ => Ok(()),
    //     }
    // }
    //
    // fn mouse_button_up_event(
    //         &mut self,
    //         ctx: &mut Context,
    //         button: event::MouseButton,
    //         _x: f32,
    //         _y: f32,
    //     ) -> Result<(), ggez::GameError> {
    //     match button {
    //         event::MouseButton::Left => set_cursor_grabbed(ctx, false),
    //         _ => Ok(()),
    //     }
    // }
    //
    // fn mouse_motion_event(
    //         &mut self,
    //         ctx: &mut Context,
    //         x: f32,
    //         y: f32,
    //         dx: f32,
    //         dy: f32,
    //     ) -> Result<(), ggez::GameError> {
    //     if cursor_grabbed(ctx) {
    //         let res = ctx.gfx.drawable_size();
    //         let scale = ctx.gfx.window().scale_factor() as f32;
    //         set_position(ctx, Vec2::new(700., 350.) / scale)?;
    //     }
    //     Ok(())
    // }
}

pub fn main() -> GameResult {
    std::fs::create_dir_all("images").unwrap();
    let capture_count = {
        let mut i = 0;
        loop {
            let path = format!("images/nearfield_{:02}.png", i);
            if !Path::new(&path).exists() {
                break i;
            }
            i += 1;
        }
    };
    let cb = ggez::ContextBuilder::new("super_simple", "ggez")
        .window_setup(WindowSetup {
            title: "donger viewer".into(),
            ..Default::default()
        })
        .window_mode(WindowMode {
            width: 1400.,
            height: 750.,
            ..Default::default()
        });
    let (mut ctx, event_loop) = cb.build()?;
    let state = MainState::new(&mut ctx, capture_count)?;
    event::run(ctx, event_loop, state)
}

fn reader_thread(
    path: String,
    wf_data: Arc<Mutex<PajData>>,
    nf_data: Arc<Mutex<PajData>>,
    // img_channel: SyncSender<(Port, [u8; 98*98])>,
    detector_params: Arc<DetectorParams>,
    upside_down: bool,
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
                if buf[buf.len()-6] == byte {
                    shiftr(&mut buf, i as u8+1);
                }
            }
        }

        let board_cols = detector_params.cols.load(Ordering::Relaxed);
        let board_rows = detector_params.rows.load(Ordering::Relaxed);
        let pattern = detector_params.pattern.load(Ordering::Relaxed);
        let gray: &[u8; 98*98] = &buf[..98*98].try_into().unwrap();
        let pattern_points = match (pattern, id) {
            (DetectorPattern::None, _) => None,
            (DetectorPattern::Chessboard, 0) => { chessboard::get_chessboard_corners(&gray, Port::Wf, board_rows, board_cols, false, upside_down) },
            (DetectorPattern::Chessboard, _) => { chessboard::get_chessboard_corners(&gray, Port::Nf, board_rows, board_cols, false, upside_down) },
            (DetectorPattern::Acircles, 0) => { circles::get_circles_centers(&gray, Port::Wf, board_rows, board_cols, false, upside_down) },
            (DetectorPattern::Acircles, _) => { circles::get_circles_centers(&gray, Port::Nf, board_rows, board_cols, false, upside_down) },
        };

        let mut paj_data = if id == 0 {
            // let _ = img_channel.try_send((Port::Wf, buf[..98*98].try_into().unwrap()));
            wf_data.lock().unwrap()
        } else {
            // let _ = img_channel.try_send((Port::Nf, buf[..98*98].try_into().unwrap()));
            nf_data.lock().unwrap()
        };
        for i in 0..98 * 98 {
            paj_data.image[4 * i] = buf[i];
            paj_data.image[4 * i + 1] = buf[i];
            paj_data.image[4 * i + 2] = buf[i];
        }
        paj_data.gray = *gray;
        for (i, mut raw_mot) in buf[98*98..][..256].chunks(16).enumerate() {
            paj_data.mot_data[i] = MotData::parse(&mut raw_mot);
        }
        paj_data.avg_frame_period = paj_data.avg_frame_period * 7. / 8. + paj_data.last_frame.elapsed().as_secs_f64() / 8.;
        paj_data.last_frame = Instant::now();
        paj_data.pattern_points = pattern_points.map(|v| v.into());
        drop(paj_data);
    }
}

fn opencv_thread(raw_image: Receiver<(Port, [u8; 98*98])>, detector_params: Arc<DetectorParams>, upside_down: bool) {
    loop {
        let (port, image) = raw_image.recv().unwrap();
        let board_cols = detector_params.cols.load(Ordering::Relaxed);
        let board_rows = detector_params.rows.load(Ordering::Relaxed);
        let pattern = detector_params.pattern.load(Ordering::Relaxed);
        match (pattern, port) {
            (DetectorPattern::None, _) => (),
            (DetectorPattern::Chessboard, Port::Wf) => { chessboard::get_chessboard_corners(&image, Port::Wf, board_rows, board_cols, true, upside_down); },
            (DetectorPattern::Chessboard, Port::Nf) => { chessboard::get_chessboard_corners(&image, Port::Nf, board_rows, board_cols, true, upside_down); },
            (DetectorPattern::Acircles, Port::Wf) => { circles::get_circles_centers(&image, Port::Wf, board_rows, board_cols, true, upside_down); },
            (DetectorPattern::Acircles, Port::Nf) => { circles::get_circles_centers(&image, Port::Nf, board_rows, board_cols, true, upside_down); },
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
