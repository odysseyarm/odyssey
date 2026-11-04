//! The camera backend and the stream it opens: per-frame noise, infrared
//! rendering and the IMU on top of the static [`View`].

use std::fmt;
use std::time::Duration;

use image::{GrayImage, Luma, Rgb, RgbImage};
use nalgebra::{Point2, Vector3};
use rand::rngs::StdRng;
use rand::{Rng as _, SeedableRng as _};

use super::scene::{SyntheticScene, Truth};
use super::view::{DepthBase, View};
use crate::camera::{
    CameraBackend, CameraError, CameraGeometry, CameraInfo, CameraStream, CaptureSettings,
    DepthImage, Frames, ImuSample, Link,
};

const GRAVITY: f64 = 9.81;
const IMU_RATE_HZ: f64 = 200.0;
const FRAME_RATE_HZ: f64 = 30.0;
const DEPTH_UNIT_M: f32 = 0.001;

/// How long the camera is taken to have been streaming when it opens, so that
/// the IMU already has history.
const WARM_UP: Duration = Duration::from_millis(600);

/// Standard deviation of the accelerometer noise per axis, in the units of
/// [`GRAVITY`].
const ACCEL_NOISE_STD: f64 = 0.03;

/// The gyroscope's constant offset per axis.
const GYRO_BIAS: [f64; 3] = [0.001, -0.0005, 0.0007];

/// Standard deviation of the gyroscope noise per axis.
const GYRO_NOISE_STD: f64 = 0.002;

/// Infrared exposures up to this many microseconds count as short: only the
/// marker LEDs stay bright.
const SHORT_IR_EXPOSURE_US: u32 = 1000;

/// Infrared background as `(floor, span)`: every pixel is drawn uniformly from
/// `floor..floor + span` levels. A short exposure leaves almost nothing.
const IR_BACKGROUND_SHORT: (f64, f64) = (3.0, 3.0);
const IR_BACKGROUND_AUTO: (f64, f64) = (45.0, 12.0);

/// Share of infrared pixels lit by the depth projector's speckle, and the
/// levels they add. The speckle is invisible at a short exposure or with the
/// projector off.
const PROJECTOR_SPECKLE_SHARE: f64 = 0.02;
const PROJECTOR_SPECKLE_LEVEL: f64 = 60.0;

/// A marker LED renders as a Gaussian spot of this standard deviation in
/// pixels, drawn out to [`MARKER_SPOT_HALF_SIZE_PX`] around its center; the
/// peak is brighter than the sensor's maximum so the core saturates.
const MARKER_SPOT_SIGMA_PX: f64 = 1.6;
const MARKER_SPOT_HALF_SIZE_PX: i32 = 7;
const MARKER_SPOT_PEAK: f64 = 255.0 * 1.8;

/// Standard-normal sample by the Box-Muller transform.
fn gaussian(rng: &mut StdRng) -> f64 {
    let u1: f64 = rng.random_range(f64::EPSILON..1.0);
    let u2: f64 = rng.random();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// Independent Gaussian noise of standard deviation `sigma` on each axis.
fn gaussian_vector(rng: &mut StdRng, sigma: f64) -> Vector3<f64> {
    Vector3::new(
        sigma * gaussian(rng),
        sigma * gaussian(rng),
        sigma * gaussian(rng),
    )
}

/// Lists one synthetic camera and opens it.
#[derive(Debug, Clone, Default)]
pub struct SyntheticBackend {
    pub scene: SyntheticScene,
}

impl SyntheticBackend {
    pub fn new(scene: SyntheticScene) -> Self {
        Self { scene }
    }

    pub fn truth(&self) -> Truth {
        self.scene.truth()
    }
}

impl CameraBackend for SyntheticBackend {
    fn list(&self) -> Result<Vec<CameraInfo>, CameraError> {
        Ok(vec![CameraInfo {
            id: "synthetic".into(),
            name: "Synthetic depth camera".into(),
            serial: "SYNTHETIC-0".into(),
            link: Link::Unknown,
        }])
    }

    fn open(
        &self,
        _camera: &CameraInfo,
        settings: &CaptureSettings,
    ) -> Result<Box<dyn CameraStream>, CameraError> {
        Ok(Box::new(SyntheticStream::new(self.scene.clone(), settings)))
    }
}

/// A running synthetic camera.
pub struct SyntheticStream {
    view: View,
    color_base: RgbImage,
    depth_base: DepthBase,
    marker_px: Vec<Point2<f64>>,
    rng: StdRng,
    clock: Duration,
    next_imu: Duration,
    imu: Vec<ImuSample>,
    ir_exposure_low: bool,
    projector_on: bool,
}

impl fmt::Debug for SyntheticStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyntheticStream")
            .field("scene", &self.view.scene)
            .field("clock", &self.clock)
            .field("ir_exposure_low", &self.ir_exposure_low)
            .field("projector_on", &self.projector_on)
            .finish_non_exhaustive()
    }
}

impl SyntheticStream {
    pub fn new(scene: SyntheticScene, settings: &CaptureSettings) -> Self {
        let mut rng = StdRng::seed_from_u64(scene.seed);
        let view = View::new(scene, settings);
        let color_base = view.render_color();
        let depth_base = view.render_depth_base(&mut rng);
        let marker_px = view.project_markers();
        let mut stream = Self {
            view,
            color_base,
            depth_base,
            marker_px,
            rng,
            clock: Duration::ZERO,
            next_imu: Duration::ZERO,
            imu: Vec::new(),
            ir_exposure_low: false,
            projector_on: true,
        };
        stream.advance_clock(WARM_UP);
        stream
    }

    pub fn scene(&self) -> &SyntheticScene {
        &self.view.scene
    }

    fn render_ir(&mut self) -> GrayImage {
        let intr = self.view.geometry.ir;
        let (w, h) = (intr.width, intr.height);
        let (floor, span) = if self.ir_exposure_low {
            IR_BACKGROUND_SHORT
        } else {
            IR_BACKGROUND_AUTO
        };
        let mut img = GrayImage::new(w, h);
        for p in img.pixels_mut() {
            let mut v = floor + self.rng.random::<f64>() * span;
            if self.projector_on
                && !self.ir_exposure_low
                && self.rng.random::<f64>() < PROJECTOR_SPECKLE_SHARE
            {
                v += PROJECTOR_SPECKLE_LEVEL;
            }
            *p = Luma([v as u8]);
        }
        let sigma = MARKER_SPOT_SIGMA_PX;
        for c in &self.marker_px {
            let half = MARKER_SPOT_HALF_SIZE_PX;
            for dy in -half..=half {
                for dx in -half..=half {
                    let (x, y) = ((c.x.round() as i32 + dx), (c.y.round() as i32 + dy));
                    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                        continue;
                    }
                    let d2 = (f64::from(x) - c.x).powi(2) + (f64::from(y) - c.y).powi(2);
                    let add = MARKER_SPOT_PEAK * (-d2 / (2.0 * sigma * sigma)).exp();
                    let cur = f64::from(img.get_pixel(x as u32, y as u32)[0]);
                    img.put_pixel(x as u32, y as u32, Luma([(cur + add).min(255.0) as u8]));
                }
            }
        }
        img
    }

    fn render_depth(&mut self) -> DepthImage {
        let scene = &self.view.scene;
        let (w, h) = (
            self.view.geometry.depth.width,
            self.view.geometry.depth.height,
        );
        let mut img = DepthImage::new(w, h);
        let (sigma, holes) = (scene.depth_noise_m, scene.depth_hole_fraction);
        for i in 0..(w * h) as usize {
            let base = f64::from(self.depth_base.depth_m[i]) * (1.0 + scene.depth_scale_error);
            if base <= 0.0 || (holes > 0.0 && self.rng.random::<f64>() < holes) {
                continue;
            }
            let noisy = if self.depth_base.persistent_outlier[i] {
                base
            } else {
                base + sigma * gaussian(&mut self.rng)
            };
            let raw = (noisy / f64::from(DEPTH_UNIT_M))
                .round()
                .clamp(1.0, f64::from(u16::MAX)) as u16;
            img.as_mut()[i] = raw;
        }
        img
    }

    fn render_color_frame(&mut self) -> RgbImage {
        let mut img = self.color_base.clone();
        if self.view.scene.color_noise > 0.0 {
            // Uniform noise on `-a..=a` has standard deviation `a / sqrt(3)`.
            let amplitude = self.view.scene.color_noise * 3.0f64.sqrt();
            for p in img.pixels_mut() {
                let n = self.rng.random_range(-amplitude..=amplitude);
                let v = (f64::from(p[0]) + n).round().clamp(0.0, 255.0) as u8;
                *p = Rgb([v; 3]);
            }
        }
        img
    }

    /// Moves the camera's clock forward and records the IMU samples that fall
    /// in between.
    fn advance_clock(&mut self, by: Duration) {
        self.clock += by;
        if !self.view.scene.imu_enabled {
            return;
        }
        let down = self.view.scene.down_in_camera();
        let period = Duration::from_secs_f64(1.0 / IMU_RATE_HZ);
        while self.next_imu <= self.clock {
            let accel = -down * GRAVITY + gaussian_vector(&mut self.rng, ACCEL_NOISE_STD);
            let gyro = Vector3::from(GYRO_BIAS) + gaussian_vector(&mut self.rng, GYRO_NOISE_STD);
            self.imu.push(ImuSample {
                timestamp: self.next_imu,
                gyro,
                accel,
            });
            self.next_imu += period;
        }
    }
}

impl CameraStream for SyntheticStream {
    fn geometry(&self) -> &CameraGeometry {
        &self.view.geometry
    }

    fn next_frames(&mut self, _timeout: Duration) -> Result<Frames, CameraError> {
        let interval = Duration::from_secs_f64(1.0 / FRAME_RATE_HZ);
        if self.view.scene.realtime {
            std::thread::sleep(interval);
        }
        self.advance_clock(interval);
        let color = self.render_color_frame();
        let depth = self.render_depth();
        let ir = self.render_ir();
        Ok(Frames {
            color,
            depth,
            depth_unit_m: DEPTH_UNIT_M,
            ir,
        })
    }

    fn drain_imu(&mut self) -> Vec<ImuSample> {
        std::mem::take(&mut self.imu)
    }

    fn set_ir_exposure_us(&mut self, exposure_us: u32) -> Result<(), CameraError> {
        self.ir_exposure_low = exposure_us <= SHORT_IR_EXPOSURE_US;
        Ok(())
    }

    fn set_ir_auto_exposure(&mut self) -> Result<(), CameraError> {
        self.ir_exposure_low = false;
        Ok(())
    }

    fn set_projector_enabled(&mut self, enabled: bool) -> Result<bool, CameraError> {
        self.projector_on = enabled;
        Ok(true)
    }
}
