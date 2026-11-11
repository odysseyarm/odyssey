use arrayvec::ArrayVec;
use nalgebra::{Isometry3, Matrix3, Matrix3x1, Point2, Rotation3};
use odyssey_cv::foveated::FoveatedAimpointState;
use odyssey_usb::packets::vm::legacy::MotData;

#[derive(Debug)]
pub struct MotState {
    // Normalized to 0.0..=1.0.
    pub fv_aimpoint: Point2<f32>,

    pub distance: f32,

    pub nf_data: Option<ArrayVec<MotData, 16>>,
    pub wf_data: Option<ArrayVec<MotData, 16>>,

    pub nf_points: ArrayVec<(u8, Point2<f32>), 16>,
    pub wf_points: ArrayVec<(u8, Point2<f32>), 16>,
    pub nf_markers: ArrayVec<Point2<f32>, 16>,
    pub wf_markers: ArrayVec<Point2<f32>, 16>,
    pub wf_reproj: ArrayVec<Point2<f32>, 16>,
    pub nf_markers2: ArrayVec<Marker, 16>,
    pub wf_markers2: ArrayVec<Marker, 16>,

    /// Set when the markers came from a legacy `CombinedMarkersReport`
    /// (4095x4095) rather than a `MarkersReport` (PAG7665QN, 320x240 with 6
    /// fractional bits, so at most 20416x15296).
    pub legacy_markers: bool,

    pub screen_id: u8,
    pub orientation: Rotation3<f32>,
    pub madgwick: ahrs::Madgwick<f32>,

    pub rotation_mat: Matrix3<f32>,
    pub translation_mat: Matrix3x1<f32>,

    pub fv_state: FoveatedAimpointState,
    pub fv_zero_offset: Isometry3<f32>,

    fv_aimpoint_history: [(Point2<f32>, f32, Matrix3x1<f32>); 80],
    fv_aimpoint_history_index: usize,
}

impl Default for MotState {
    fn default() -> Self {
        Self {
            fv_aimpoint: Point2::new(0.0, 0.0),
            distance: 0.0,
            nf_data: None,
            wf_data: None,
            legacy_markers: true,
            screen_id: 0,
            orientation: Rotation3::identity(),
            madgwick: ahrs::Madgwick::new(1. / 100., 0.04),
            rotation_mat: Matrix3::identity(),
            translation_mat: Default::default(),
            nf_points: Default::default(),
            wf_points: Default::default(),
            nf_markers: Default::default(),
            wf_markers: Default::default(),
            wf_reproj: Default::default(),
            nf_markers2: Default::default(),
            wf_markers2: Default::default(),
            fv_state: FoveatedAimpointState::new(),
            fv_zero_offset: Isometry3::identity(),
            fv_aimpoint_history: [(Point2::new(0.0, 0.0), 0., Matrix3x1::new(0.0, 0.0, 0.0)); 80],
            fv_aimpoint_history_index: 0,
        }
    }
}

impl MotState {
    pub(crate) fn push_aimpoint(&mut self, cant: f32) {
        self.fv_aimpoint_history[self.fv_aimpoint_history_index] =
            (self.fv_aimpoint, cant, self.translation_mat);
        self.fv_aimpoint_history_index =
            (self.fv_aimpoint_history_index + 1) % self.fv_aimpoint_history.len();
    }

    /// The cursor denotes the next write, so an impact uses the preceding slot.
    pub(crate) fn latest_aimpoint(&self) -> (Point2<f32>, f32, Matrix3x1<f32>) {
        let index = (self.fv_aimpoint_history_index + self.fv_aimpoint_history.len() - 1)
            % self.fv_aimpoint_history.len();
        self.fv_aimpoint_history[index]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Marker {
    pub mot_id: u8,
    pub pattern_id: Option<u8>,
    pub normalized: Point2<f32>,
}

impl Marker {
    pub fn odyssey_cv_marker(&self) -> odyssey_cv::foveated::Marker {
        odyssey_cv::foveated::Marker {
            position: self.normalized.cast(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impacts_use_latest_aimpoint_before_and_after_history_wraps() {
        let mut state = MotState::default();
        for sample in 1..=81 {
            let value = sample as f32;
            state.fv_aimpoint = Point2::new(value, -value);
            state.translation_mat = Matrix3x1::new(value, value + 1.0, value + 2.0);
            state.push_aimpoint(value + 3.0);
            assert_eq!(
                state.latest_aimpoint(),
                (
                    Point2::new(value, -value),
                    value + 3.0,
                    Matrix3x1::new(value, value + 1.0, value + 2.0),
                )
            );
        }
    }
}
