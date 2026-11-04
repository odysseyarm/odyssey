# Tracking regression and bench evidence

The regression runner executes the production hub tracker, not a second tracking
implementation. Synthetic scenes provide independently projected observations and
known truth. Limits are engineering regression limits, not measured hardware
accuracy specifications.

## Run locally

```sh
cargo run -p odyssey-hub-server --features tracking-regression --bin tracking-regression -- suite tracking-report.json
cargo run -p odyssey-hub-server --features tracking-regression --bin tracking-regression -- generate /tmp/tracking-fixtures
```

Use the project's development shell. `suite` exits nonzero on regression; JSON
reports retain metrics and observations. CI runs the same suite and contract tests
on PRs, main/master pushes, manual dispatch and nightly, and uploads its report.
CI needs the existing `ODYSSEY_CV_TOKEN` secret for the private solver dependency.
`Cargo.lock` pins the solver Git commit; builds do not require a sibling checkout
or a manifest `rev`. The solver's lockfile also pins its shared calibration
dependency. Publish referenced commits before CI fetches them. Odyssey supplies
its matching `odyssey-common` through the root patch.

CLI operations after building the binary:

```sh
tracking-regression replay TRACE.cbor REPORT.json
tracking-regression capture TRACE.cbor CAPTURE.cbor
tracking-regression evaluate TRACE.cbor CAPTURE.cbor REPORT.json
tracking-regression evaluate-report TRACE.cbor REPORT.json RECHECKED.json
tracking-regression import-recording SESSION.bin TEMPLATE.json TRACE.cbor
tracking-regression scenes /tmp/tracking-scenes
tracking-regression validate-scene SCENE.json
tracking-regression validate-trace TRACE.cbor
tracking-regression surface-suite surface-report.json
tracking-regression validate-asset SCENE.glb
tracking-regression relabel-scene INPUT.json OUTPUT.json --asset SCENE.glb
tracking-regression measure-marker-noise PHYSICAL.cbor MODEL.json
tracking-regression apply-marker-noise SYNTHETIC.cbor MODEL.json SEED NOISY.cbor
tracking-regression dump FILE.cbor
```

Output files must not already exist. `capture` runs production replay and exports
the resulting capture; external backends can write the same versioned contract.
`evaluate` verifies trace name and SHA256 before scoring. These commands do not
control a motorized bench or connect to a reference tracker.

Traces and captures are CBOR only; generated fixtures use `.cbor`. Reports,
scenes, noise models and recording templates are JSON only. File extensions never
select an output encoding. Both readers reject unknown and duplicate fields and
bound input to 64 MiB. CBOR preserves f32 values exactly. `dump` reads CBOR traces
or captures and prints JSON for inspection, not reimport. A capture binds to SHA256
over its trace's canonical CBOR encoding; for a trace written by this tool it
equals `sha256sum` of the file. `evaluate-report` revalidates the report's embedded
capture against the original trace and recomputes metrics; supplied report metrics
are never trusted. Unreleased schemas remain at version 1. When a later field
changes how existing data must be read, add a `requires` list that readers check.

Existing `odyssey-vision-tool` `record` binary records physical USB sensor packets.
`import-recording` reuses `odyssey-playback` to read that format. Its separate JSON
template contains `name`, `source` (`id` and six-byte `device_uuid`), `screens` and
`thresholds`. It contains no device settings, samples, scene or truth; settings
come from the recording. Import preserves thresholds: importing with accuracy
limits must not silently waive accuracy requirements. Add independently
measured truth, or explicitly select structural-only thresholds (zero
`min_truth_matches`, null accuracy limits) and retain `not_accuracy_checked` status.
Legacy arrival clocks are wall-clock milliseconds, normalized to microseconds;
equal timestamps retain order, backward clocks and overflow are rejected.

## Shared software / hardware boundary

A trace contains calibration, source identity and clock provenance, ordered raw
sensor observations, and optional independent truth. Arrival `elapsed_us` is not
the IMU's wrapping device timestamp. Preserve both. Replay has no wall-clock
sleeps: timing comes from recorded sensor data, not machine speed.

The same evaluator accepts captured tracking outputs from another backend or a
bench run. Capture must identify the exact input trace. Never reuse labels from
one run with observations from another. A report with no independent truth cannot
certify accuracy. Comparing against previous software output is useful for change
detection, but does not turn that output into physical ground truth.

For bench sessions, retain device firmware and hardware revision, calibration,
clock synchronization method and uncertainty, raw inputs, output capture, and
reference-system provenance with the trace. Measure reference pose independently
using a calibrated fixture or reference tracker. Align reference and device clocks
before computing latency or motion error. A host receipt timestamp alone cannot
measure sensor-to-output latency.

For mixed HIL/simulation, retain distinct source IDs and clock descriptions for
physical and synthetic inputs. Merge inputs on an explicitly established arrival
timebase, keep device timestamps unchanged, and record which channels were
replaced. Each replayed device must use its matching calibration. Source metadata
is not clock synchronization and does not emulate USB, BLE or sensor electrical
behavior.

For physical IMU plus simulated optical observations on one logical unit, give
both sources the same `device_uuid` and identical settings, but distinct `id` and
`clock_basis` values. They feed one tracker. Different UUIDs keep independent
tracker state. A reset from either source resets their shared device state.

## Rendered scenes and experimental tracking models

Scene descriptions carry camera geometry and poses separately from generated
sensor observations. Renderer output can use matching scene/frame/camera IDs to
associate images with pose and aim labels. Projection geometry and reference truth
must not be derived from the tracking solver's output.

Each scene screen stores its actual `ScreenCalibration`: rotation, homography,
six ordered marker positions and an optional measured `surface`. Marker IDs refer
to the marker order; there is no separate guessed screen size or duplicate marker
geometry. Without a surface, the display is the `z = 0` plane bounded by the
inverse homography. A surface is a row-major grid of physical positions
(`columns`, `rows`, `positions_m`) with uniform normalized screen UVs; each cell
splits along its top-left to bottom-right diagonal. Nonuniform spacing represents
nonlinear projector mapping; depth variation represents waviness. Coordinates are
physical screen meters; calibration rotation maps world vectors into that frame.

The surface lives in `odyssey-common`, so the tracker, the simulator and the
tools share one implementation. The tracker (`odyssey-cv`) maps aim by
intersecting the aim ray with the surface when present, and zeroing targets
through it. If the ray misses the surface it falls back to the plane, which keeps
off-screen aim values. The homography stays the planar fit used for marker
identification. Calibrations without a surface behave exactly as before.

Zero targets use the same calibrated surface as aim raycasts, including the hub's
tracking-event path; they are not projected onto the fitted plane.

`surface-suite` replays four display cases (`calibration-plane`,
`projective-plane`, `wavy-grid`, `nonlinear-projector-grid`) through the
production tracker, with markers on the surface at their calibrated UVs and
mesh-derived truth. All four use the default limits. A test replays each grid case
with the surface removed from the tracker's calibration and requires the error
to drop at least twofold with the surface present. These are simulator numbers,
not hardware accuracy. Measuring a real display's surface is not implemented; the
desktop calibration wizard still produces the planar fit only.

General scene assets use glTF 2.0 GLB, not an Odyssey-specific environment schema.
Blender's native exporter preserves mesh topology, UVs, materials, transforms and
custom identity/calibration extras; Rust's `gltf` crate reads the same asset.
The domain-specific JSON retains exact camera intrinsics/distortion, sensor clocks,
calibration mapping and reference labels. GLB and `.blend` files do not replace
trace/capture identity or the evaluator.

The image manifest binds the GLB by SHA256 and records its transform back into
Odyssey world coordinates. Blender's standard glTF Y-up export maps back as
`[x, y, z] -> [x, -z, y]`. Assets are bounded, self-contained static GLB files;
unsupported dynamic geometry is rejected rather than scored at a guessed pose.
`relabel-scene` explicitly recomputes synthetic aim/position/visibility labels after
editing geometry; `--asset` includes environment mesh occlusion.

Images stay in external artifacts rather than bloating sensor trace CBOR. An image
reference is not evidence that the classical tracker processed that image.

Training and held-out evaluation must split by scene/session/device, not adjacent
frames, to avoid leaking near-identical images between sets. Preserve units,
coordinate transforms, exposure intervals and label uncertainty. Real sensor noise,
saturation, blur, rolling shutter and missed detections need measured models before
synthetic performance can support hardware accuracy claims.

An experimental transformer should produce the same capture contract, tagged with
model/checkpoint and preprocessing provenance, then use the same evaluator. It must
remain opt-in and must not replace the default tracker through this runner. This
change includes no transformer inference, training stack or calibrated sensor renderer.

### Blender authoring and rendering

The isolated tool shell pins Blender, the Rerun SDK and native viewer without
adding them to production Rust dependencies:

```sh
nix-shell tools/tracking_scene/shell.nix
export VALIDATOR="$PWD/target/debug/tracking-regression"
tracking-regression scenes /tmp/tracking-scenes
blender -b --python-exit-code 1 --python tools/tracking_scene/blender_scene.py -- \
  --validator "$VALIDATOR" build /tmp/tracking-scenes/wavy-grid.json /tmp/wavy.blend
blender -b --python-exit-code 1 --python tools/tracking_scene/blender_scene.py -- \
  --validator "$VALIDATOR" render /tmp/wavy.blend /tmp/wavy-render
blender -b --python-exit-code 1 --python tools/tracking_scene/blender_scene.py -- \
  --validator "$VALIDATOR" export /tmp/wavy.blend /tmp/wavy-edited.json
```

Build the regression binary first and add its directory to `PATH`. Output files
and render directories must be new. Edit calibrated screen vertices and marker
positions in Blender; preserve their IDs, UVs and grid topology. General static
environment meshes travel through native GLB rather than custom primitive types.
Author camera trajectories and intrinsics in scene JSON; unsupported camera edits
are rejected rather than silently discarded.

Cameras follow OpenCV intrinsics, including skew and Brown-Conrady distortion
(k1, k2, p1, p2, k3). Blender renders an ideal pinhole view with overscan, then
every output pixel is remapped through the inverse distortion (the same
fixed-point inversion as OpenCV `undistortPoints`). Distortion that cannot be
inverted across the image, or that needs more than 4x overscan, is rejected. A
nonzero `exposure_us` averages 16 linear-light renders across the exposure
interval, with the camera pose interpolated between authored frames; an exposure
that runs past the camera's last authored pose is rejected. Global shutter only:
no rolling shutter. The selfcheck compares rendered marker centroids with an
independent distorted projection, and a motion streak's centroid and length
with the mid-exposure pose. Projective planes use a tessellated inverse-homography
UV approximation; the Rust geometric oracle retains the exact homography.

### Sensor models

The camera module reports marker centroids, so blur, saturation and thresholding
reach the tracker as centroid jitter and missed detections.
`measure-marker-noise` estimates both from any trace, including an imported
physical recording: per-axis centroid sigma from second differences across
consecutive reports (smooth motion cancels, white noise leaves 6 sigma²), and
the dropout rate of markers seen in the reports on either side. Measure a still or
slowly moving recording; fast acceleration inflates sigma. `apply-marker-noise`
adds a model to a clean synthetic trace with a fixed seed, accounting for the
rounding already in the reports, so measuring the result returns the model. Truth
and expectations are unchanged and provenance records the model and seed.

For rendered images, a camera's optional `sensor` sets a Gaussian point spread
(`blur_sigma_px`), `full_well_electrons` (rendered linear intensity 1.0 fills the
well and brighter light saturates), `read_noise_electrons`, ADC `bit_depth` and
`seed`. Renders then get Poisson shot noise, Gaussian read noise, full-well
clipping and quantization, and are stored as linear sensor counts (`encoding:
linear_sensor_counts` in the manifest, 16-bit PNG above 8 bits) instead of
sRGB. Noise is seeded per frame, so renders repeat. The selfcheck compares the
stored image with the model and checks the normalized noise has unit spread.
Model parameters must come from measurements of the real sensor; with guessed
values the images are still not evidence of hardware behaviour. Rolling shutter,
dark current, fixed-pattern noise and per-pixel gain are not modeled.

### Rerun inspection

```sh
python3 tools/tracking_scene/rerun_export.py TRACE.cbor OUTPUT.rrd \
  --capture CAPTURE.cbor --validator "$VALIDATOR" --self-check
python3 tools/tracking_scene/rerun_export.py SCENE.json SCENE.rrd \
  --images /tmp/wavy-render/manifest.json --validator "$VALIDATOR" --self-check
rerun OUTPUT.rrd
```

The exporter displays calibrated surfaces and marker identities, camera geometry,
reference/estimated tracking, error/validity and optional rendered images/GLB.
It validates capture identity through the Rust evaluator. Sensor timestamps,
arrival time and scene exposure time remain distinct; visual alignment does not
establish clock synchronization. Rerun is not the pass/fail oracle or canonical
recording format. No viewer dependency is required for headless regression or
bench scoring.

Headless screenshots of a file argument can quit before rerun 0.38.1 finishes
registering the recording; stream it instead:
`cat OUTPUT.rrd | rerun --headless --screenshot-to out.png -`. Checked with
software Vulkan (`VK_ICD_FILENAMES` pointing at Mesa's `lvp_icd` file) and with
the NVIDIA ICD, where wgpu selected the discrete GPU.

## Release gate

Run deterministic regressions on every change. Keep representative real recordings
and add each reproduced field bug as a fixture. Require absolute product limits as
well as comparisons against the shipped version; a baseline can preserve a bug.

Before customer rollout, run supported boards and host OSes on a bench, including
tracking loss/reacquisition, transport transitions, image-mode restoration, real
updates from deployed versions, interrupted update recovery, and calibration
retention. Use internal/canary devices before the remaining customer fleet. Passing
software tests does not replace these hardware checks.
