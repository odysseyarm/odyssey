//! The wizard's stages and where each sits on the stepper.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Stage {
    Setup,
    Position,
    Measuring,
    Markers,
    MeasuringMarkers,
    Review,
    Saved,
    Problem,
}

impl Stage {
    pub(crate) const STEPS: [&'static str; 5] =
        ["Setup", "Position", "Measure", "Markers", "Review"];

    /// Index into [`Self::STEPS`]; `Saved` is one past the end so every step
    /// shows as done.
    pub(super) fn step(self) -> usize {
        match self {
            Self::Setup => 0,
            Self::Position => 1,
            Self::Measuring => 2,
            Self::Markers | Self::MeasuringMarkers => 3,
            Self::Review => 4,
            Self::Saved => 5,
            Self::Problem => 0,
        }
    }
}
