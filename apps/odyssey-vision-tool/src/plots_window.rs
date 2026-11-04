use std::collections::VecDeque;
use std::f64::consts::PI;
use std::ops::Range;
use std::sync::{MutexGuard, PoisonError};
use std::time::Instant;

use eframe::egui::{Color32, Frame, Key, Rect, RichText, ScrollArea, Sense, Ui, UiBuilder, vec2};
use egui_plot::{Corner, Legend, Line, Plot, PlotPoint};
use nalgebra::Vector3;
use odyssey_cv::telemetry::Series;

/// Number of chart rows the plots window is divided into.
const PLOT_ROWS: usize = 6;

/// Number of chart columns the plots window is divided into.
const PLOT_COLUMNS: usize = 2;

/// Charts are never squeezed below this height; the window scrolls instead.
const MIN_CELL_HEIGHT: f32 = 150.0;

/// Width of the y axis label area, so plots in a column line up.
const Y_AXIS_MIN_WIDTH: f32 = 34.0;

/// Fraction of the y range left empty above and below the data.
const Y_MARGIN_FRACTION: f32 = 0.05;

/// Line width of every trace, in points.
const TRACE_WIDTH: f32 = 1.5;

/// Lighter than the pure primaries so they stay readable on the dark theme.
const RED: Color32 = Color32::from_rgb(235, 87, 87);
const GREEN: Color32 = Color32::from_rgb(87, 200, 110);
const BLUE: Color32 = Color32::from_rgb(92, 150, 255);

/// One plotted line: its legend name and color.
#[derive(Debug)]
struct Trace {
    name: &'static str,
    color: Color32,
}

const XYZ: &[Trace] = &[
    Trace {
        name: "x",
        color: RED,
    },
    Trace {
        name: "y",
        color: GREEN,
    },
    Trace {
        name: "z",
        color: BLUE,
    },
];

const COUNT: &[Trace] = &[Trace {
    name: "count",
    color: RED,
}];

/// The points of every trace of one chart. Scalar charts only fill the first.
type Traces = [Vec<PlotPoint>; 3];

/// A chart: what to plot, and how to read it from the telemetry.
#[derive(Debug)]
struct Chart {
    title: &'static str,
    /// The y axis always shows at least this range, and grows to fit the data.
    y_range: Range<f64>,
    traces: &'static [Trace],
    /// Replaces the points in `out` with the current samples and returns the
    /// number of samples the series keeps, which is the width of the x axis.
    fill: fn(out: &mut Traces) -> usize,
}

const GYRO: Chart = Chart {
    title: "IMU Angular Velocity",
    y_range: -0.03..0.03,
    traces: XYZ,
    fill: |out| {
        let series = odyssey_cv::telemetry::imu_data();
        fill_vec3(series, out, |sample| sample.1.cast())
    },
};

const ACCEL: Chart = Chart {
    title: "IMU Acceleration",
    y_range: -10.0..10.0,
    traces: XYZ,
    fill: |out| {
        let series = odyssey_cv::telemetry::imu_data();
        fill_vec3(series, out, |sample| sample.0.cast())
    },
};

const ACCEL_BIAS: Chart = Chart {
    title: "ESKF Accel. Bias",
    y_range: -5.0..5.0,
    traces: XYZ,
    fill: |out| fill_vec3_f32(odyssey_cv::telemetry::eskf_accel_bias(), out),
};

const LOCAL_GRAVITY: Chart = Chart {
    title: "ESKF Local Gravity",
    y_range: -10.0..10.0,
    traces: XYZ,
    fill: |out| fill_vec3_f32(odyssey_cv::telemetry::eskf_local_gravity(), out),
};

const APPARENT_ACCEL: Chart = Chart {
    title: "ESKF Apparent Accel.",
    y_range: -10.0..10.0,
    traces: XYZ,
    fill: |out| fill_vec3_f32(odyssey_cv::telemetry::eskf_apparent_accel(), out),
};

const VELOCITY: Chart = Chart {
    title: "ESKF Velocity",
    y_range: -1.0..1.0,
    traces: XYZ,
    fill: |out| fill_vec3_f32(odyssey_cv::telemetry::eskf_velocity(), out),
};

const ACCEL_BIAS_UNCERTAINTY: Chart = Chart {
    title: "ESKF Accel. Bias Uncertainty",
    y_range: 0.0..1.0,
    traces: XYZ,
    fill: |out| fill_vec3_f32(odyssey_cv::telemetry::eskf_accel_bias_uncertainty(), out),
};

const VELOCITY_UNCERTAINTY: Chart = Chart {
    title: "ESKF Velocity Uncertainty",
    y_range: 0.0..1.0,
    traces: XYZ,
    fill: |out| fill_vec3_f32(odyssey_cv::telemetry::eskf_velocity_uncertainty(), out),
};

const ORIENTATION_UNCERTAINTY: Chart = Chart {
    title: "ESKF Orientation Uncertainty",
    y_range: 0.0..10.0,
    traces: XYZ,
    fill: |out| {
        let series = odyssey_cv::telemetry::eskf_orientation_uncertainty();
        // The telemetry is in radians; the chart is in degrees.
        fill_vec3(series, out, |sample| sample.cast() * 180.0 / PI)
    },
};

const ESKF_MISMATCH: Chart = Chart {
    title: "ESKF/P3P Mismatch Count",
    y_range: 0.0..10.0,
    traces: COUNT,
    fill: |out| {
        let series = odyssey_cv::telemetry::eskf_mismatch_count();
        let values = locked_values(series);
        out[0].clear();
        out[0].extend(
            values
                .iter()
                .enumerate()
                .map(|(i, sample)| PlotPoint::new(index_to_x(i), f64::from(sample.1))),
        );
        series.size
    },
};

const POSITION_UNCERTAINTY: Chart = Chart {
    title: "ESKF Position Uncertainty",
    y_range: 0.0..1.0,
    traces: XYZ,
    fill: |out| fill_vec3_f32(odyssey_cv::telemetry::eskf_position_uncertainty(), out),
};

const POSITION: Chart = Chart {
    title: "ESKF Position",
    y_range: 0.0..5.0,
    traces: XYZ,
    fill: |out| fill_vec3_f32(odyssey_cv::telemetry::eskf_position(), out),
};

/// The charts shown, row by row. Its length is the grid size.
const CHARTS: [&Chart; PLOT_ROWS * PLOT_COLUMNS] = [
    &GYRO,
    &ACCEL,
    &ESKF_MISMATCH,
    &APPARENT_ACCEL,
    &ACCEL_BIAS,
    &LOCAL_GRAVITY,
    &VELOCITY_UNCERTAINTY,
    &VELOCITY,
    &POSITION_UNCERTAINTY,
    &POSITION,
    &ACCEL_BIAS_UNCERTAINTY,
    &ORIENTATION_UNCERTAINTY,
];

/// The samples of a telemetry series. A poisoned lock only means another
/// thread panicked mid-update; the plots can still show what is there.
fn locked_values<T>(series: &Series<T>) -> MutexGuard<'_, VecDeque<(Instant, T)>> {
    series.values.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The x coordinate of the `index`th sample: the x axis counts samples.
const fn index_to_x(index: usize) -> f64 {
    index as f64
}

/// Replaces the points in `out` with `series` mapped through `to_vec3`.
/// Returns the number of samples the series keeps.
fn fill_vec3<T>(
    series: &Series<T>,
    out: &mut Traces,
    to_vec3: impl Fn(&T) -> Vector3<f64>,
) -> usize {
    let [xs, ys, zs] = out;
    xs.clear();
    ys.clear();
    zs.clear();
    for (i, sample) in locked_values(series).iter().enumerate() {
        let x = index_to_x(i);
        let v = to_vec3(&sample.1);
        xs.push(PlotPoint::new(x, v.x));
        ys.push(PlotPoint::new(x, v.y));
        zs.push(PlotPoint::new(x, v.z));
    }
    series.size
}

fn fill_vec3_f32(series: &Series<Vector3<f32>>, out: &mut Traces) -> usize {
    fill_vec3(series, out, |sample| sample.cast())
}

/// A chart and the points it currently shows.
#[derive(Debug)]
struct ChartData {
    chart: &'static Chart,
    points: Traces,
    sample_count: usize,
}

impl ChartData {
    fn new(chart: &'static Chart) -> Self {
        Self {
            chart,
            points: Traces::default(),
            sample_count: 0,
        }
    }

    fn refresh(&mut self) {
        self.sample_count = (self.chart.fill)(&mut self.points);
    }

    /// Draws the title and the plot, filling the space available in `ui`.
    fn show(&self, ui: &mut Ui) {
        let chart = self.chart;
        ui.label(RichText::new(chart.title).strong());

        let mut plot = Plot::new(chart.title)
            // Static view: the x axis always spans the whole history and the
            // y axis always fits the data, so there is nothing to pan or zoom.
            .allow_drag(false)
            .allow_zoom(false)
            .allow_scroll(false)
            .allow_boxed_zoom(false)
            .allow_double_click_reset(false)
            .set_margin_fraction(vec2(0.0, Y_MARGIN_FRACTION))
            .include_x(0.0)
            .include_x(index_to_x(self.sample_count))
            .include_y(chart.y_range.start)
            .include_y(chart.y_range.end)
            .show_grid([false, true])
            .y_axis_min_width(Y_AXIS_MIN_WIDTH);
        if chart.traces.len() > 1 {
            plot = plot.legend(
                Legend::default()
                    .position(Corner::RightTop)
                    .background_alpha(0.5),
            );
        }
        plot.show(ui, |plot_ui| {
            for (trace, points) in chart.traces.iter().zip(&self.points) {
                plot_ui.line(
                    Line::new(trace.name, points.as_slice())
                        .color(trace.color)
                        .width(TRACE_WIDTH),
                );
            }
        });
    }
}

/// Live telemetry charts in a grid that fills the window. Space pauses and
/// resumes them.
#[derive(Debug)]
pub struct PlotsWindow {
    /// While set, the charts keep showing the data they had when paused.
    paused: bool,
    charts: Vec<ChartData>,
}

impl Default for PlotsWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl PlotsWindow {
    pub fn new() -> Self {
        Self {
            paused: false,
            charts: CHARTS.iter().copied().map(ChartData::new).collect(),
        }
    }

    pub fn show(&mut self, ui: &mut Ui) {
        if ui.input(|i| i.key_released(Key::Space)) {
            self.paused = !self.paused;
        }
        if !self.paused {
            for chart in &mut self.charts {
                chart.refresh();
            }
        }

        ui.horizontal(|ui| {
            ui.weak("Space pauses and resumes the plots");
            if self.paused {
                ui.colored_label(Color32::ORANGE, "Paused");
            }
        });

        ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| self.show_grid(ui));
    }

    /// Lays the charts out in equally sized cells that fill the available space.
    fn show_grid(&self, ui: &mut Ui) {
        let spacing = ui.spacing().item_spacing;
        let available = ui.available_size();
        let cell = vec2(
            (available.x - spacing.x * (PLOT_COLUMNS - 1) as f32) / PLOT_COLUMNS as f32,
            ((available.y - spacing.y * (PLOT_ROWS - 1) as f32) / PLOT_ROWS as f32)
                .max(MIN_CELL_HEIGHT),
        );
        let grid_size = vec2(
            available.x,
            (cell.y + spacing.y) * PLOT_ROWS as f32 - spacing.y,
        );
        let (grid_rect, _) = ui.allocate_exact_size(grid_size, Sense::hover());

        for (index, chart) in self.charts.iter().enumerate() {
            let offset = vec2(
                (index % PLOT_COLUMNS) as f32 * (cell.x + spacing.x),
                (index / PLOT_COLUMNS) as f32 * (cell.y + spacing.y),
            );
            let cell_rect = Rect::from_min_size(grid_rect.min + offset, cell);
            ui.scope_builder(UiBuilder::new().max_rect(cell_rect), |ui| {
                Frame::group(ui.style()).show(ui, |ui| chart.show(ui));
            });
        }
    }
}
