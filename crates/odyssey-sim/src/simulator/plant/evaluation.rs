//! Fixed exposure against the adaptive loop, with and without the hub's hint, over
//! scripted scenes. Everything is seeded, so the numbers are the same on every run.

use odyssey_protocol::wire::CameraCalibrationParams;
use odyssey_protocol::{GeneralConfig, MarkersReport};

use super::super::unit::reset_registers;
use super::super::world::World;
use super::{Distractor, EXPOSURE, EXPOSURE_MASK, Sensor, Source};
use crate::simulator::SensorModel;

const FRAME_MS: u64 = 10;
const HINT_PERIOD_MS: u64 = 100;
/// With the stock 120 the exposure window around the stored 20 cannot reach 6 m.
const LIGHT_THRESHOLD: u8 = 60;
const GRID: [u32; 2] = SensorModel::Pag7665qn.report_grid();
const WINDOW: Distractor = Distractor::Window {
    px: [1920.0, 14080.0],
    intensity: 50.0,
};
const LAMP: Distractor = Distractor::Lamp {
    px: [18880.0, 1280.0],
    intensity: 1.0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Fixed,
    Adaptive,
    Hinted,
}

struct Phase {
    name: String,
    from_m: f64,
    to_m: f64,
    intensity: f64,
    ms: u64,
    move_ms: u64,
    /// Frames before this are settling and are not scored.
    measure_from_ms: u64,
}

impl Phase {
    fn hold(name: &str, distance_m: f64, intensity: f64, ms: u64, measure_from_ms: u64) -> Self {
        Self {
            name: name.to_owned(),
            from_m: distance_m,
            to_m: distance_m,
            intensity,
            ms,
            move_ms: 0,
            measure_from_ms,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Metrics {
    visible: u32,
    detected: u32,
    saturated: u32,
    squared_error: f64,
    exposure: u8,
}

impl Metrics {
    fn detection(&self) -> f64 {
        f64::from(self.detected) / f64::from(self.visible.max(1))
    }

    fn saturation(&self) -> f64 {
        f64::from(self.saturated) / f64::from(self.visible.max(1))
    }

    /// Native px; positions are in report units.
    fn rms_px(&self) -> f64 {
        (self.squared_error / f64::from(self.detected.max(1))).sqrt() / MarkersReport::UNITS_PER_PX
    }
}

struct Run {
    phases: Vec<Metrics>,
    exposures: Vec<u8>,
    saw_window: bool,
}

fn camera() -> GeneralConfig {
    GeneralConfig::CameraModelNf(
        CameraCalibrationParams {
            camera_matrix: SensorModel::Pag7665qn.camera_matrix(),
            dist_coeffs: [0.0; 5],
        }
        .into(),
    )
}

fn simulate(mode: Mode, distractors: &[Distractor], phases: &[Phase]) -> Run {
    let world = World::synthetic();
    let camera = camera();
    let base = world.aim_pose([0.5, 0.5]).unwrap();
    let mut registers = reset_registers(SensorModel::Pag7665qn);
    registers.insert((0, 0x6d), LIGHT_THRESHOLD);
    let mut sensor = Sensor::new(mode != Mode::Fixed);
    sensor.distractors = distractors.to_vec();

    let mut run = Run {
        phases: Vec::new(),
        exposures: Vec::new(),
        saw_window: false,
    };
    let mut now_ms = 0;
    let mut matched: Vec<[u16; 2]> = Vec::new();
    for phase in phases {
        let mut metrics = Metrics::default();
        sensor.led_intensity = phase.intensity;
        for elapsed_ms in (0..phase.ms).step_by(FRAME_MS as usize) {
            let progress = if phase.move_ms == 0 {
                1.0
            } else {
                (elapsed_ms as f64 / phase.move_ms as f64).min(1.0)
            };
            let distance_m = phase.from_m + (phase.to_m - phase.from_m) * progress;
            let mut pose = base.clone();
            pose.position_m[2] = -distance_m;
            let markers = world.targets(&pose, &camera, GRID);

            if mode == Mode::Hinted && now_ms % HINT_PERIOD_MS == 0 && !matched.is_empty() {
                sensor.hint(&matched, now_ms);
            }
            let frame = sensor.frame(&mut registers, now_ms, &markers);
            run.exposures.push(registers[&EXPOSURE] & EXPOSURE_MASK);
            metrics.exposure = registers[&EXPOSURE] & EXPOSURE_MASK;
            if let Some(frame) = &frame {
                run.saw_window |= frame.spots.iter().any(|s| s.source == Source::Window);
                matched = frame
                    .spots
                    .iter()
                    .filter(|s| matches!(s.source, Source::Marker(_)))
                    .map(|s| [s.px[0].round() as u16, s.px[1].round() as u16])
                    .collect();
            }

            if elapsed_ms >= phase.measure_from_ms {
                metrics.visible += markers.len() as u32;
                for (index, marker) in markers.iter().enumerate() {
                    let seen = frame
                        .iter()
                        .flat_map(|f| &f.spots)
                        .find(|s| s.source == Source::Marker(index));
                    let Some(seen) = seen else { continue };
                    metrics.detected += 1;
                    metrics.saturated += u32::from(seen.peak >= 255.0);
                    metrics.squared_error +=
                        (seen.px[0] - marker.px[0]).powi(2) + (seen.px[1] - marker.px[1]).powi(2);
                }
            }
            now_ms += FRAME_MS;
        }
        run.phases.push(metrics);
    }
    run
}

fn sweep() -> Vec<Phase> {
    let mut phases = vec![Phase::hold("1 m", 1.0, 1.0, 5000, 3000)];
    for metres in 2..=6 {
        phases.push(Phase {
            name: format!("{metres} m"),
            from_m: f64::from(metres - 1),
            to_m: f64::from(metres),
            intensity: 1.0,
            ms: 7000,
            move_ms: 2000,
            measure_from_ms: 5000,
        });
    }
    phases
}

fn print(scenario: &str, phases: &[Phase], runs: &[(Mode, &Run)]) {
    println!("\n{scenario}");
    println!(
        "{:<8} {:<9} {:>7} {:>6} {:>8} {:>4}",
        "phase", "mode", "detect", "sat", "rms px", "exp"
    );
    for (index, phase) in phases.iter().enumerate() {
        for (mode, run) in runs {
            let m = &run.phases[index];
            println!(
                "{:<8} {:<9} {:>6.1}% {:>5.1}% {:>8.3} {:>4}",
                phase.name,
                format!("{mode:?}"),
                m.detection() * 100.0,
                m.saturation() * 100.0,
                m.rms_px(),
                m.exposure
            );
        }
    }
}

#[test]
fn adaptive_exposure_holds_detection_across_a_distance_sweep() {
    let phases = sweep();
    let fixed = simulate(Mode::Fixed, &[], &phases);
    let adaptive = simulate(Mode::Adaptive, &[], &phases);
    let hinted = simulate(Mode::Hinted, &[], &phases);
    print(
        "distance sweep, 1 to 6 m",
        &phases,
        &[
            (Mode::Fixed, &fixed),
            (Mode::Adaptive, &adaptive),
            (Mode::Hinted, &hinted),
        ],
    );

    for (index, phase) in phases.iter().enumerate() {
        let floor = fixed.phases[index].detection();
        for (mode, run) in [(Mode::Adaptive, &adaptive), (Mode::Hinted, &hinted)] {
            assert!(
                run.phases[index].detection() >= floor,
                "{mode:?} at {}: {} < fixed {floor}",
                phase.name,
                run.phases[index].detection()
            );
        }
    }
    let mean =
        |run: &Run| run.phases.iter().map(Metrics::detection).sum::<f64>() / phases.len() as f64;
    assert!(
        mean(&adaptive) > mean(&fixed) + 0.2,
        "{} vs {}",
        mean(&adaptive),
        mean(&fixed)
    );
    assert!(mean(&adaptive) >= 0.95, "{}", mean(&adaptive));

    let (near_fixed, near_adaptive) = (&fixed.phases[0], &adaptive.phases[0]);
    assert!(near_fixed.saturation() > 0.9, "{}", near_fixed.saturation());
    assert!(near_adaptive.saturation() < near_fixed.saturation());
    assert!(
        near_adaptive.saturation() < 0.05,
        "{}",
        near_adaptive.saturation()
    );
    // A blown-out blob's centroid is biased as well as noisy.
    assert!(near_adaptive.rms_px() < near_fixed.rms_px());

    let again = simulate(Mode::Adaptive, &[], &phases);
    assert_eq!(
        adaptive.exposures, again.exposures,
        "the same seed must give the same run"
    );
}

#[test]
fn adaptive_exposure_follows_led_intensity() {
    let phases = [
        Phase::hold("I x1", 3.0, 1.0, 5000, 3000),
        Phase::hold("I x0.5", 3.0, 0.5, 6000, 4000),
        Phase::hold("I x4", 3.0, 4.0, 8000, 6000),
    ];
    let fixed = simulate(Mode::Fixed, &[], &phases);
    let adaptive = simulate(Mode::Adaptive, &[], &phases);
    print(
        "LED intensity change at 3 m",
        &phases,
        &[(Mode::Fixed, &fixed), (Mode::Adaptive, &adaptive)],
    );

    for index in 0..phases.len() {
        assert!(adaptive.phases[index].detection() >= fixed.phases[index].detection());
    }
    assert!(adaptive.phases[1].detection() > fixed.phases[1].detection());
    assert!(adaptive.phases[2].saturation() < fixed.phases[2].saturation());
    assert!(adaptive.phases[2].exposure < adaptive.phases[1].exposure);
}

#[test]
fn a_window_is_in_the_frame_but_never_measured() {
    let phases = [Phase::hold("window", 3.0, 1.0, 6000, 4000)];
    let without = simulate(Mode::Adaptive, &[], &phases);
    let with = simulate(Mode::Adaptive, &[WINDOW], &phases);
    print(
        "window, 3 m",
        &phases,
        &[(Mode::Adaptive, &without), (Mode::Adaptive, &with)],
    );

    assert!(
        with.saw_window,
        "the sensor must report the window for the test to mean anything"
    );
    assert_eq!(
        with.exposures, without.exposures,
        "a window that moved the exposure was measured"
    );
    assert_eq!(with.phases[0].detected, without.phases[0].detected);
}

#[test]
fn a_round_lamp_is_measured_without_the_hint_and_excluded_with_it() {
    let phases = [Phase::hold("lamp", 3.0, 1.0, 8000, 5000)];
    let fixed = simulate(Mode::Fixed, &[LAMP], &phases);
    let adaptive = simulate(Mode::Adaptive, &[LAMP], &phases);
    let hinted = simulate(Mode::Hinted, &[LAMP], &phases);
    print(
        "round lamp, 3 m",
        &phases,
        &[
            (Mode::Fixed, &fixed),
            (Mode::Adaptive, &adaptive),
            (Mode::Hinted, &hinted),
        ],
    );

    // Chasing the lamp starves the markers.
    assert!(adaptive.phases[0].exposure < hinted.phases[0].exposure);
    assert!(
        adaptive.phases[0].detection() < 0.5,
        "{}",
        adaptive.phases[0].detection()
    );
    assert!(
        hinted.phases[0].detection() >= 0.95,
        "{}",
        hinted.phases[0].detection()
    );
    assert!(hinted.phases[0].detection() >= fixed.phases[0].detection());
}

#[test]
fn an_ideal_sensor_reports_the_projected_markers_exactly() {
    let world = World::synthetic();
    let camera = camera();
    let pose = world.aim_pose([0.3, 0.6]).unwrap();
    let markers = world.targets(&pose, &camera, GRID);
    let mut registers = reset_registers(SensorModel::Pag7665qn);
    let mut sensor = Sensor::new(false);
    sensor.ideal = true;
    let frame = sensor.frame(&mut registers, 0, &markers).unwrap();

    assert_eq!(markers.len(), 6);
    for (index, marker) in markers.iter().enumerate() {
        let spot = frame
            .spots
            .iter()
            .find(|s| s.source == Source::Marker(index))
            .unwrap();
        assert_eq!(spot.px, marker.px);
    }
    assert_eq!(frame.report(), world.markers(&pose, &camera, GRID));
}

#[test]
fn an_exposure_write_blinds_the_sensor_for_a_frame_gap() {
    let world = World::synthetic();
    let markers = world.targets(&world.aim_pose([0.5, 0.5]).unwrap(), &camera(), GRID);
    let mut registers = reset_registers(SensorModel::Pag7665qn);
    registers.insert((0, 0x68), 6);
    let mut sensor = Sensor::new(true);
    // Twice the stock brightness: the first frame asks for less exposure.
    assert!(sensor.frame(&mut registers, 1000, &markers).is_some());
    assert_eq!(
        registers[&(0, 0x67)] & 0x80,
        0x80,
        "LED always-on survives the write"
    );
    assert!(registers[&(0, 0x67)] & 0x7f < 20);
    assert!(sensor.frame(&mut registers, 1010, &markers).is_none());
    assert!(sensor.frame(&mut registers, 1020, &markers).is_some());
}

#[test]
fn off_sensor_lights_do_not_change_exposure() {
    let phases = [Phase::hold("outside", 3.0, 1.0, 3000, 0)];
    let baseline = simulate(Mode::Adaptive, &[], &phases);
    for px in [[-1000.0, 1000.0], [f64::from(GRID[0]) + 1000.0, 1000.0]] {
        let outside = simulate(
            Mode::Adaptive,
            &[Distractor::Lamp { px, intensity: 1.0 }],
            &phases,
        );
        assert_eq!(outside.exposures, baseline.exposures, "{px:?}");
        assert_eq!(outside.phases[0].detected, baseline.phases[0].detected);
    }
}
