## Board flashing

`cargo xtask firmware <board> flash` uses `probe-rs download` with read-back
verification and reset. Probe-rs selects flash algorithms for both nRF5340 cores
from addresses in the merged HEX. Normal flashing erases affected sectors; dual-core
boards also clear boot-state pages. `flash-chiperase` erases the whole chip.
`flash-recover` explicitly permits erasing protected memory, including settings
and security keys. `flash-app` writes only the application; `reset` uses probe-rs.
Nordic `mergehex` remains a build-time packaging tool, not a flashing backend.

## Live image inspection and calibration

Calibration runs inside `odyssey-vision-tool`; `viewer` is now a reusable
calibration library, not a separate capture application. Open **Image / camera
calibration** (or the sensor settings' capture action), start the USB preview,
capture target views, then solve and export or apply the calibration. Calibration
uses the main window; **Back to tracking** stops the preview. On modern cameras
tracking resumes immediately; on legacy PAJ7025 it still requires recovery below.
Mono results include RMS and per-view errors. Stereo calibration consumes paired
NF/WF observations. Solving uses native image pixels; exported and applied camera
matrices are scaled into the selected device's tracking coordinates.

In Odyssey desktop, open **Image / IR** on a USB-connected device to inspect the
native grayscale image, observed FPS and receive age. Image capture is image-only:
neither application displays object diagnostics or centroid overlays. Calibration
target corners remain visible in vision-tool.

Both applications request **15 FPS** by default, adjustable from **1–15 FPS**.
This is a requested rate, not a measured sensor maximum. Acquisition and bounded
USB transfers can lower the observed rate; firmware does not clamp requests to
5 FPS or add a full frame interval after transfer time.

Modern ATSlite captures 320×240 NF images without rebooting. The sensor stays in
image mode throughout capture; optical tracking pauses. Stop, stream drop, USB
disconnect or capture failure restores live sensor settings and object mode.
IMU and control traffic remain independent. Images are USB-only; BLE requests
are rejected explicitly.

Modern capture is leased. Start grants 5 s; the viewer renews every second while
it is still consuming frames (the hub client renews from the frame stream's poll,
vision-tool from its frame loop; the hub never renews on its own). Without a
same-generation renewal the firmware ends capture, restores tracking and reports
`LeaseExpired`, even with USB still attached, so a crashed or hung viewer or hub
cannot leave the camera in image mode. Older firmware lacks the lease and must be
updated. A firmware hang is outside the lease's reach. Legacy ignores renewals and
keeps its reboot recovery.

Legacy PAJ7025 ATSlite/VM uses the recovered sensor-master image entry procedure
and Nordic SPIS receiver to acquire native **98×98 NF or WF** grayscale pixels
(up to 15 requested FPS). The trailing sensor transport bytes are not pixels.
No object samples are acquired during image capture. Image configuration is temporary:
entering/stopping capture never flashes over the saved tracking settings.

**Legacy capture requires reboot/power-cycle and reconnect to restore tracking.**
Both UIs warn before entry. **Stop**, dialog close, capture failure and USB
disconnect terminate pixel reception, not sensor image mode. A generation-bound
Stop returns `RebootRequired`; a stale Stop cannot stop the current generation.
Further capture/settings operations are refused until recovery. The firmware
never recreates a register SPI master or tries an undocumented reverse register
sequence while the sensor drives its SPI pins.

In vision-tool, choose **Stop preview & reboot legacy device**. In Odyssey desktop,
choose **Stop images & reboot legacy device** in **Image / IR**; closing the dialog
retains a recovery warning on the device row. These use the existing USB control
reboot acknowledgement, not DFU or firmware flashing. Wait for a fresh USB
connection; acknowledgement alone does not prove tracking has recovered. In
vision-tool reconnect/reselect the same unit in configuration before applying or
flashing calibration. Successful connection/settings reads unlock those actions;
NF/WF captures and fits are retained for the same device UUID across reconnects,
so collecting the other camera can continue after recovery.

If the control reboot is unavailable (including VM hardware without a control
endpoint), or the unit does not return to tracking, fully power-cycle it and
reconnect. Unplugging USB alone may not turn off a battery-powered ATSlite.
Saved settings return at boot; reapply any unsaved sensor edits after recovery.
The original image firmware used this reboot workflow; it does not establish a
safe reboot-free image-master → register-slave transition.

Legacy ATSlite retains its fixed dual-bank flash layout. Its target configuration
uses `-Z location-detail=none` to omit runtime panic source paths while retaining
DWARF debug information for probe-rs. Build with the pinned nightly and
`cargo xtask firmware legacy-atslite build-app-a` / `build-app-b`, which rebuild the
standard library with size optimization; no bank boundaries or BLE features are changed.

The calibration library requires native OpenCV. The Nix development shell supplies
OpenCV and clang include paths; Windows ChArUco detection uses
[OpenCV 4.9.0](https://github.com/opencv/opencv/releases/download/4.9.0/opencv-4.9.0-windows.exe).
Both applications offer explicit simulator mode when built with the `sim` feature
and launched with `--sim` (modern) or `--sim-legacy` (PAJ7025 reboot lifecycle).
Images are explicitly synthetic, not sensor observations. The legacy profile
exposes native NF/WF images without object samples, requires reboot after Stop,
and reconnects with a new USB endpoint; modern simulation remains reboot-free.