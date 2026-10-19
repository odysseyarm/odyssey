# Tracker markers: what the wizard does, what the tracker needs, what is open

The wizard measures where the six infrared marker LEDs are (in meters, in the screen's
plane) and writes them to `ScreenCalibration::object_points`. The hub's tracker then finds
the markers in the controller's camera by their geometry. This note says what each side
requires. It was written from reading the code; only the items marked CONFIRMED were read
in code, and nothing here was run against the tracker.

## What the wizard does today

`markers.rs` takes the bright spots of the infrared image, puts each on the screen plane and
assigns the six nearest to `MARKER_LAYOUT` (fractions of the screen), within 10% of the
screen diagonal. Markers mounted anywhere else are not recognised, and the markers step says
"N of 6 markers are where they should be". The order of `object_points` is the order of
`MARKER_LAYOUT`.

## What the tracker needs

The hub links `odyssey-cv` (rev 2700dae in `Cargo.lock`), not `ats_cv`; the identification code
is the same in both. File references are to `crates/odyssey-cv/src/foveated.rs` there.

- CONFIRMED, all six at once (`:652`, `:752`): tracking starts only when every one of the six
  `object_points` matches a visible spot. Extra bright spots are tolerated unless they land
  where a marker is expected.
- CONFIRMED, a straight row of three (`:647`, `:674-692`): `object_points[1]` and `[2]` must be
  the ends of a nearly straight row with exactly one other marker between them (the middle
  point's angle must be at least about 143 degrees), and no other marker may lie near that line.
  `MARKER_LAYOUT` has it as `1 5 2` along the bottom (`0 3 4` is the upper row; the diagram is in
  `apps/odyssey-vision-tool/src/bin/screeninfogen.rs`).
- CONFIRMED, the order is the contract (`:700-724`): the pose seed is a P3P solve on
  `object_points[0..3]` against image points chosen by those tests, and every candidate is then
  checked by reprojecting all six. A wrong order matches nothing.
- CONFIRMED, thresholds (`:641-644`): reprojection errors of 0.01 (P3P, summed squared
  normalized-image distance) and 0.009 (PnP); a pose whose gravity disagrees with the
  calibration by more than about 11.5 degrees is dropped.
- CONFIRMED, with more than one calibrated screen, tracking holds only while the aim point is
  within 2% of the screen's width (`helpers.rs:88,95`).

## Open: not verified, so not shown to users

- **Handedness and vertical order of the seed (`:695`, `:700-706`).** The tests require the
  cross product of `(object_points[0] - [1])` and `([2] - [1])` to be non-positive in image
  coordinates, and `[1]` not to be below `[0]` along gravity. Read literally with y down,
  `MARKER_LAYOUT` fails both. The shipped layout presumably works, so some convention (a
  flipped image, a calibration `rotation`, the sign of the hub's gravity vector) makes them
  agree, and it is not visible in code. Until that is settled, an order other than
  `MARKER_LAYOUT`'s must not be assumed to work.
- **The "danger cylinder".** It is not checked anywhere in the tracker. The only
  position-limit code is commented out or dead (`:102-110`, `:645-646`, `:804-808`). If the
  P3P danger cylinder (the cylinder through the three seed points, perpendicular to their
  plane) is to guide placement, the rule and the numbers have to come from whoever owns the
  tracker; none are written down here.
- **Markers elsewhere than `MARKER_LAYOUT`** (for example around the screen's edge, or behind a
  projector screen). Accepting them needs an order the tracker accepts; see the first point.

## How to settle it

Run the tracker's own `identify_markers_from_screen_calibration` on synthetic views of the
points the wizard produces, with the gravity vector the hub builds, and check which order it
accepts. That needs `odyssey-cv`, which this repo fetches over an SSH URL that CI cannot reach
(see the comment above `exclude` in the workspace `Cargo.toml`), so the test belongs in a crate
that already depends on it (`odyssey-hub-server` or `odyssey-vision-tool`), not here.
