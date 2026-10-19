# Tracker markers: where they can go, and the order the tracker needs

The wizard measures where the six infrared marker LEDs are (meters, in the screen's plane,
x to the right, y down from the top-left corner) and writes them into
`ScreenCalibration::object_points`. The hub's tracker then finds the markers in the controller's
camera by their geometry.

## What the wizard does

`markers.rs` takes the bright spots of the infrared image, puts each on the screen plane, and
keeps those on the screen or within 25 % of its size beyond an edge (markers sit on the bezel or
behind a projector screen). Exactly six must be there. `placement.rs` then orders them for the
tracker. No fixed layout is assumed. A frame that cannot be ordered says so in the markers step
("the markers can't be told apart"), and the log gets a plain account of the spots.

## The order is the contract

The tracker (`odyssey-cv`, rev 2700dae in `Cargo.lock`; `foveated.rs`,
`identify_markers_from_screen_calibration`) takes `object_points[0..3]` as the seed of a three
point pose solve, and finds the matching image points by testing every triple:

- `[1]` and `[2]` are the ends of a nearly straight row with exactly one other marker between
  them (the tracker counts a point as between when the angle at it is wider than about 143
  degrees). No other marker may count as between.
- `[0]` lies below `[1]` along gravity, and the cross product of `[0] - [1]` and `[2] - [1]` is
  not positive in the image (y down): the row's right end is `[2]`.
- `[3]`, `[4]` and `[5]` take no part in the seed. The tracker matches them by reprojection, so
  any order of them works.
- Every candidate is then checked against all six, and by a pose solve over all six.

`placement::tracker_order` applies those tests to the screen's frame, with margins (a middle
marker at straightness 0.9 rather than 0.8, every other marker at 0.6 or less, `[0]` at least 5 %
of the markers' extent below `[1]`, 15 degrees off the row), picks the widest such triple, and puts
the marker between the row's ends fourth and the other two in order of x. The margins are ours, not
the tracker's; they make an order that holds in the controller's image, not only in the screen's
frame, and they can refuse an arrangement the tracker would accept.

## What was run, not assumed

`crates/odyssey-hub-server/tests/marker_order.rs` builds a `ScreenCalibration` and runs the
tracker's own `identify_markers_from_screen_calibration` on synthetic views: 45 camera positions
(five positions in front of the screen, three distances from 1.5 to 4 m, three rolls), gravity as
the tracker expects it, the points shuffled. Results:

- The original Python autocalib's pattern (`screen-autocalib/mathstuff.py`, `marker_pattern`:
  `[0.2, 1], [0.2, 0], [0.8, 0], [0.5, 0], [0.5, 1], [0.8, 1]` of the screen, three along the top
  edge and three along the bottom) is identified in every view, in that order. 18 of its 720
  orders are: `[1]` and `[2]` the top row's ends, `[0]` any of the bottom three, the rest in any
  order.
- Swapping `[1]` and `[2]`, or putting the middle marker first, is not identified.
- The order `tracker_order` picks is identified in every view for the pattern in any input order,
  for markers on a TV's bezel (from a real log), with 2 and 5 mm of measurement error, mirrored,
  and for markers well inside the picture (behind a projector screen).
- With a row along the bottom and the others scattered above it, `tracker_order` finds no order and
  the tracker identifies none either.
- Standing on the circle through the seed's three markers (the "danger cylinder" of a three point
  solve) is a weak spot and not a limit: about nine in ten such positions still lock on, and
  positions off the circle (0.6 and 1.15 of its radius) all do. The review lists the circle's
  centre and radius.

## Found on the way

The wizard used to look for its markers at `[0.18, 0.29], [0.15, 0.82], [0.77, 0.80], [0.51, 0.35],
[0.79, 0.25], [0.49, 0.76]` of the screen, in that order. That is the vision tool's "for serious
wall" pattern (`screeninfogen.rs`), a single installation, and not the pattern of the original
autocalib. In that order, with the camera orientation the tests assume (see Open), the tracker
identified it in none of the 45 views. A calibration saved by an earlier version of the wizard has
its markers stored in that order, so by this test it would not track; it should be redone with this
version, and checked with the hub if it was ever used.

## Open

- The tests use a camera with x right, y down, as the Python pattern and the tracker's own
  documentation (gravity "towards the ground, positive y") do. The orientation of the real
  tracker camera's images was not checked against hardware.
- Only the tracker's identification was run, not its pose filter over time. With more than one
  calibrated screen the tracker also drops lock when the aim point is more than 2 % outside the
  screen's width (`helpers.rs`); that was read, not run.
- The numbers behind "usually" for the seed circle come from exact synthetic data. Noise may move
  them.
