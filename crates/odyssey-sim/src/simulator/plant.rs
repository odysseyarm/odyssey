//! Object-mode plant of the PAG7665QN tracking chip behind a PAG7646J2 sensor.
//!
//! Each frame turns the LEDs and distractors in view into the spots the chip
//! would report from its exposure, gain, light threshold and area registers.
//! The real [`ExposureController`] closes the loop through register 0x67, so a
//! simulated unit adapts exactly as the firmware does.
//!
//! Spot positions are on the chip's report grid, 1/64 native px, which is what
//! `MarkersReport`, hub hints and the controller all use.

use std::f64::consts::{PI, TAU};

use nalgebra::Point2;
use odyssey_cv::exposure::{ExposureController, Limits, Spot};
use odyssey_protocol::MarkersReport;

use super::unit::Registers;
use super::world::Marker;

const EXPOSURE: (u8, u8) = (0, 0x67);
const GAIN: (u8, u8) = (0, 0x68);
const LIGHT_THRESHOLD: (u8, u8) = (0, 0x6d);
const AREA_MIN: [(u8, u8); 2] = [(0, 0x6e), (0, 0x6f)];
const AREA_MAX: [(u8, u8); 2] = [(0, 0x70), (0, 0x71)];
const LED_ALWAYS_ON: u8 = 0x80;
const EXPOSURE_MASK: u8 = 0x7f;

/// The chip's report grid: 320x240 native px interpolated to 1/64 px.
const GRID: [f64; 2] = [20480.0, 15360.0];
const UNITS_PER_PX: f64 = 64.0;
/// Datasheet gain the brightness below is stated for.
const BASE_GAIN: f64 = 3.0;
/// Peak pixel value of a unit-intensity LED at 1 m per exposure unit (100 µs) at
/// `BASE_GAIN`. Puts the default exposure of 20 at 200 counts from 2 m.
const PEAK_AT_1M: f64 = 40.0;
const SATURATION: f64 = 255.0;
/// Past this many times full scale the circle test fails on the bloomed blob.
const HEAVY_SATURATION: f64 = 3.0;
/// Image radii in native px: the LED at 1 m, and the optics' blur.
const LED_RADIUS_1M_PX: f64 = 2.0;
const BLUR_RADIUS_PX: f64 = 2.0;
/// Radius growth per e-fold of over-exposure.
const BLOOM: f64 = 1.0;
const LAMP_RADIUS_PX: f64 = 2.0;
const WINDOW_AREA_PX: f64 = 400.0;
/// Relative shot noise on a brightness reading.
const PEAK_NOISE: f64 = 0.02;
/// Centroid noise in native px at 100 counts; falls as 1/sqrt(signal).
const CENTROID_NOISE_PX: f64 = 0.06;
/// Saturated blobs bloom lopsided: centroid bias in native px per e-fold, and its cap.
const SATURATION_BIAS_PX: f64 = 0.2;
const MAX_BIAS_PX: f64 = 0.5;
/// Writing 0x67 needs Idle, which drops frames while the sensor re-enters Object.
const FRAME_GAP_MS: u64 = 20;
const MIN_DISTANCE_M: f64 = 0.05;

/// Something besides the markers the sensor sees.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "only the tests place distractors")
)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Distractor {
    /// A bright non-round blob larger than any marker.
    Window { px: [f64; 2], intensity: f64 },
    /// A marker-sized round light.
    Lamp { px: [f64; 2], intensity: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// Index into the markers the frame was made from.
    Marker(usize),
    Lamp,
    Window,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlantSpot {
    pub(crate) spot: Spot,
    pub(crate) source: Source,
    /// The centroid in report-grid units, before the chip's rounding.
    pub(crate) px: [f64; 2],
    /// Brightness before the 8-bit clip.
    pub(crate) peak: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct Frame {
    pub(crate) spots: Vec<PlantSpot>,
}

impl Frame {
    /// What `MarkersReport` carries: report-grid positions, no brightness.
    pub(crate) fn report(&self) -> MarkersReport {
        let mut points = [Point2::origin(); 16];
        let visible = self.spots.iter().filter_map(|spot| {
            let [u, v] = spot.px;
            ((1.0..=GRID[0] - 1.0).contains(&u) && (1.0..=GRID[1] - 1.0).contains(&v))
                .then(|| Point2::new(u.round() as u16, v.round() as u16))
        });
        for (slot, point) in points.iter_mut().zip(visible) {
            *slot = point;
        }
        MarkersReport { points }
    }
}

struct Settings {
    exposure: u8,
    gain: u8,
    light_threshold: u8,
    area_min: u32,
    area_max: u32,
}

impl Settings {
    fn read(registers: &Registers) -> Self {
        let get = |register: (u8, u8)| registers.get(&register).copied().unwrap_or(0);
        let pair = |[low, high]: [(u8, u8); 2]| u32::from(get(low)) | u32::from(get(high)) << 8;
        Self {
            exposure: (get(EXPOSURE) & EXPOSURE_MASK).max(1),
            gain: get(GAIN),
            light_threshold: get(LIGHT_THRESHOLD),
            area_min: pair(AREA_MIN),
            area_max: pair(AREA_MAX),
        }
    }

    /// Peak counts of a source of `intensity` seen from `distance_m`.
    fn peak(&self, intensity: f64, distance_m: f64) -> f64 {
        PEAK_AT_1M * intensity * f64::from(self.exposure) * f64::from(self.gain)
            / BASE_GAIN
            / distance_m.max(MIN_DISTANCE_M).powi(2)
    }
}

/// Counter-based noise: a draw depends only on its seed, time, source and lane,
/// so a source's noise never depends on what else is in view.
#[derive(Clone, Copy)]
struct Noise {
    seed: u64,
    time: u64,
    id: u64,
}

fn mix(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ z >> 30).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ z >> 27).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ z >> 31
}

impl Noise {
    /// In (0, 1).
    fn uniform(&self, lane: u64) -> f64 {
        let bits = mix(mix(mix(self.seed ^ self.time) ^ self.id) ^ lane) >> 11;
        (bits as f64 + 0.5) / (1_u64 << 53) as f64
    }

    /// Standard normal, from lanes `2 * lane` and `2 * lane + 1`.
    fn gauss(&self, lane: u64) -> f64 {
        let radius = (-2.0 * self.uniform(2 * lane).ln()).sqrt();
        radius * (TAU * self.uniform(2 * lane + 1)).cos()
    }
}

struct Blob {
    id: u64,
    source: Source,
    px: [f64; 2],
    peak: f64,
    /// Native px, before bloom.
    area: f64,
    round: bool,
    blooms: bool,
}

/// The sensor and chip: what the LEDs become, and the exposure loop around it.
#[derive(Debug)]
pub(crate) struct Sensor {
    pub(crate) seed: u64,
    /// No noise and no saturation bias: spots land exactly where the optics put them.
    pub(crate) ideal: bool,
    /// Scales every marker LED, as battery sag or a brightness setting does.
    pub(crate) led_intensity: f64,
    pub(crate) distractors: Vec<Distractor>,
    /// `None` keeps whatever exposure the registers hold.
    adaptive: bool,
    controller: Option<ExposureController>,
    blind_until_ms: u64,
}

impl Sensor {
    pub(crate) fn new(adaptive: bool) -> Self {
        Self {
            seed: 0x0d15_ea5e,
            ideal: false,
            led_intensity: 1.0,
            distractors: Vec::new(),
            adaptive,
            controller: None,
            blind_until_ms: 0,
        }
    }

    /// Hub hint: the report-grid positions of the spots it matched to the pattern.
    pub(crate) fn hint(&mut self, points: &[[u16; 2]], now_ms: u64) {
        let Some(controller) = &mut self.controller else {
            return;
        };
        controller.set_hint(points, now_ms as u32);
    }

    /// One sensor frame of `markers`, or `None` while the sensor is blind after
    /// an exposure write. A change the controller asks for lands in `registers`.
    pub(crate) fn frame(
        &mut self,
        registers: &mut Registers,
        now_ms: u64,
        markers: &[Marker],
    ) -> Option<Frame> {
        if now_ms < self.blind_until_ms {
            return None;
        }
        let settings = Settings::read(registers);
        if self.adaptive
            && self
                .controller
                .is_none_or(|controller| controller.exposure() != settings.exposure)
        {
            // The host wrote the register: its value is the new starting point.
            self.controller = Some(ExposureController::new(
                settings.exposure,
                Limits::around(settings.exposure),
            ));
        }

        let mut spots = Vec::with_capacity(markers.len() + self.distractors.len());
        for (index, marker) in markers.iter().enumerate() {
            let blob = Blob {
                id: index as u64,
                source: Source::Marker(index),
                px: marker.px,
                peak: settings.peak(self.led_intensity, marker.distance_m),
                area: PI
                    * ((LED_RADIUS_1M_PX / marker.distance_m.max(MIN_DISTANCE_M)).powi(2)
                        + BLUR_RADIUS_PX.powi(2)),
                round: true,
                blooms: true,
            };
            spots.extend(self.measure(&settings, now_ms, &blob));
        }
        for (index, distractor) in self.distractors.iter().enumerate() {
            let id = 1000 + index as u64;
            let blob = match *distractor {
                Distractor::Lamp { px, intensity } => Blob {
                    id,
                    source: Source::Lamp,
                    px,
                    peak: settings.peak(intensity, 1.0),
                    area: PI * (LAMP_RADIUS_PX.powi(2) + BLUR_RADIUS_PX.powi(2)),
                    round: true,
                    blooms: true,
                },
                Distractor::Window { px, intensity } => Blob {
                    id,
                    source: Source::Window,
                    px,
                    peak: settings.peak(intensity, 1.0),
                    area: WINDOW_AREA_PX,
                    round: false,
                    blooms: false,
                },
            };
            spots.extend(self.measure(&settings, now_ms, &blob));
        }

        if let Some(controller) = &mut self.controller {
            let seen: Vec<Spot> = spots.iter().map(|spot| spot.spot).collect();
            if let Some(next) = controller.update(&seen, now_ms as u32) {
                let led = registers.get(&EXPOSURE).copied().unwrap_or(0) & LED_ALWAYS_ON;
                registers.insert(EXPOSURE, led | next);
                self.blind_until_ms = now_ms + FRAME_GAP_MS;
            }
        }
        Some(Frame { spots })
    }

    fn measure(&self, settings: &Settings, now_ms: u64, blob: &Blob) -> Option<PlantSpot> {
        let noise = Noise {
            seed: self.seed,
            time: now_ms,
            id: blob.id,
        };
        let mut peak = blob.peak;
        if !self.ideal {
            peak *= 1.0 + PEAK_NOISE * noise.gauss(0);
        }
        if peak < f64::from(settings.light_threshold) {
            return None;
        }
        let over = (peak / SATURATION).max(1.0).ln();
        let area = if blob.blooms {
            blob.area * (1.0 + BLOOM * over).powi(2)
        } else {
            blob.area
        };
        let area = area.round() as u32;
        if !(settings.area_min..=settings.area_max).contains(&area) {
            return None;
        }
        let mut px = blob.px;
        if !self.ideal {
            let sigma = UNITS_PER_PX * CENTROID_NOISE_PX * (100.0 / peak.min(SATURATION)).sqrt();
            let bias = UNITS_PER_PX * (SATURATION_BIAS_PX * over).min(MAX_BIAS_PX);
            // The lopsidedness is a property of the blob, so it keeps its direction.
            let angle = TAU * Noise { time: 0, ..noise }.uniform(8);
            px[0] += sigma * noise.gauss(1) + bias * angle.cos();
            px[1] += sigma * noise.gauss(2) + bias * angle.sin();
        }
        let units = |value: f64, max: f64| value.round().clamp(0.0, max) as u16;
        Some(PlantSpot {
            spot: Spot {
                x: units(px[0], GRID[0]),
                y: units(px[1], GRID[1]),
                area,
                avg: peak.min(SATURATION).round() as u8,
                is_circle: blob.round && peak < HEAVY_SATURATION * SATURATION,
            },
            source: blob.source,
            px,
            peak,
        })
    }
}

#[cfg(test)]
mod evaluation;
