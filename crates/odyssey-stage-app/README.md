# Odyssey stage

A local web page for staging a scene: a display with real-world flaws, trackers and
dongles placed around it, light and radio conditions, and aim patterns that move the
trackers. The model lives in `odyssey-stage`; this crate serves the page and joins it
to a running hub.

```sh
cargo run -p odyssey-stage-app            # binary: odyssey-stage
cargo run -p odyssey-stage-app -- --stage room.json   # open and autosave a stage file
```

Open <http://127.0.0.1:7878>. The page listens on loopback only. Simple mode shows a
display preset, a tracker with an aim pattern, a dongle, and live radio links;
Detailed mode adds waviness, keystone and lens distortion, hand tremor and recoil,
button schedules, walls and lights, replay through the tracker, and the Blender
hand-off.

## With a hub

If a hub is running, its physical trackers appear in the stage at the position the
hub reports. Start the hub with `--sim` (a build with the `sim` feature) to add
simulated devices next to them: bind stage trackers and dongles to the "hub
simulator" and each becomes a simulated tracker or dongle of that hub. The stage
adds them, plugs the dongles in, pairs every tracker with the dongle that hears it
best (or plugs it in over USB when no dongle is bound), and from then on sends each
tracker's position and direction, trigger pulls and radio range. The hub handles
them through the same path as hardware, and the page shows the aim the hub's tracker
reports next to the staged truth.

The simulated trackers look at a display, and the hub's tracker believes in one. Two
ways to make them the same:

- "Use the hub's display" stages the display the hub is calibrated for. Use this
  with your own hub, where physical trackers are looking at that display too.
- A hub started with `--temp-config` keeps its settings in a temporary directory, so
  the stage may calibrate it: the simulated trackers then see the staged display,
  ripples included, and "The hub knows the staged display's shape" chooses whether
  the hub's tracker is told the shape or only its flat fit. Add `--no-discovery` to
  leave physical devices out of such a run.

Simulated devices never write to your settings: zero offsets and shot delays of a
simulated tracker stay in memory unless the hub runs on `--temp-config`.

## Evaluate

"Run through the real tracker" renders the staged run into raw marker and IMU
reports and replays them through the production tracker (the same replay the
regression suite uses). Running it with and without the tracker knowing the
display's shape shows what a wavy screen costs a tracker that assumes a plane.
Limits are engineering regression limits, not measured hardware accuracy.

## Blender and Rerun

Export writes the staged run as the scene file `tools/tracking_scene` reads. Build a
`.blend` from it, edit the display mesh and marker positions in Blender, export, and
import the result back: the stage then uses that display. Rerun is for inspection
after the fact (`rerun_export.py` on the scene, or on the replay's trace and capture);
it has no input widgets, so live control stays in the page. The page lists the exact
commands.

Stage files carry the patterns, so Blender and CI scenes can reuse them without the
page: `odyssey-stage --stage room.json --export-scene scene.json --device tracker-1 --fps 30`.
`--export-scenario FILE` writes the same run as an `odyssey-sim-control --scenario` file.

## What the models are

- Radio: log-distance path loss with wall losses and slow fading. Defaults come from
  the nRF52840 datasheet and the 2.44 GHz free-space loss; exponents and wall losses are
  rules of thumb. The hub does not report RSSI, so none of this is measured.
- Light: a marker is detected when its signal, falling with distance squared, clears a
  share of the infrared background; a lamp near a marker hides it. Qualitative.
- The tracker treats the six markers as a plane, so ripples stay flat along the top and
  bottom edges where the markers sit, and curved displays are not modeled.
