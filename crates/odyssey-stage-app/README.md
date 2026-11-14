# Odyssey stage

A local web page for staging a scene. You get a display with real-world flaws,
trackers and dongles placed around it, light and radio conditions, and aim
patterns that move the trackers. The model lives in `odyssey-stage`. This crate
serves the page and joins it to a running hub.

```sh
cargo run -p odyssey-stage-app            # binary: odyssey-stage
cargo run -p odyssey-stage-app -- --stage room.json   # open and autosave a stage file
```

Open <http://127.0.0.1:7878>. The page listens on loopback only. Simple mode
shows a display preset, a tracker with an aim pattern, a dongle, and live radio
links. Detailed mode adds waviness, keystone and lens distortion, hand tremor
and recoil, button schedules, walls and lights, replay through the tracker, and
the Blender hand-off.

## With a hub

If a hub is running, its physical trackers appear in the stage at the position
the hub reports. Start the hub with `--sim` (a build with the `sim` feature) to
add simulated devices next to them. Bind stage trackers and dongles to the "hub
simulator" and each becomes a simulated tracker or dongle of that hub.

The stage does the setup for you. It adds the devices, plugs the dongles in, and
pairs every tracker with the dongle that hears it best, or plugs the tracker in
over USB when no dongle is bound. From then on it sends each tracker's position
and direction, trigger pulls and radio range. The hub handles them through the
same path as hardware, and the page shows the aim the hub's tracker reports next
to the staged truth.

The simulated trackers look at a display, and the hub's tracker believes in one.
Those have to be the same display. You can get there two ways.

"Use the hub's display" stages the display the hub is calibrated for. Pick this
with your own hub, where physical trackers are looking at that display too.

The other way is a hub started with `--temp-config`. It keeps its settings in a
temporary directory, so the stage may calibrate it. The simulated trackers then
see the staged display, ripples included, and "The hub knows the staged
display's shape" chooses whether the hub's tracker is told the shape or only its
flat fit. Add `--no-discovery` to leave physical devices out of such a run.

Simulated devices never write to your settings. Zero offsets and shot delays of
a simulated tracker stay in memory unless the hub runs on `--temp-config`.

## Evaluate

"Run through the real tracker" renders the staged run into raw marker and IMU
reports and replays them through the production tracker, the same replay the
regression suite uses. Run it twice, once with the tracker knowing the display's
shape and once without, and you see what a wavy screen costs a tracker that
assumes a plane. The limits are engineering regression limits and say nothing
about measured hardware accuracy.

## Blender and Rerun

Export writes the staged run as the scene file `tools/tracking_scene` reads.
Build a `.blend` from it, edit the display mesh and marker positions in Blender,
export, and import the result back. The stage then uses that display.

Rerun is for inspection after the fact, with `rerun_export.py` on the scene or
on the replay's trace and capture. It has no input widgets, so live control
stays in the page. The page lists the exact commands.

Stage files carry the patterns, so Blender and CI scenes can reuse them without
the page: `odyssey-stage --stage room.json --export-scene scene.json --device tracker-1 --fps 30`.
`--export-scenario FILE` writes the same run as an `odyssey-sim-control --scenario` file.

## What the models are

Radio is log-distance path loss with wall losses and slow fading. The defaults
come from the nRF52840 datasheet and the 2.44 GHz free-space loss, while the
exponents and wall losses are rules of thumb. The hub does not report RSSI, so
none of this is measured.

Light is qualitative. A marker is detected when its signal, falling with
distance squared, clears a share of the infrared background. A lamp near a
marker hides it.

The tracker treats the six markers as a plane. Ripples therefore stay flat along
the top and bottom edges where the markers sit, and curved displays are not
modeled.
