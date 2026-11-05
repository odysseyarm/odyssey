## Board flashing

`cargo xtask firmware <board> flash` uses `probe-rs download` with read-back
verification and reset. Probe-rs selects flash algorithms for both nRF5340 cores
from addresses in the merged HEX. Normal flashing erases affected sectors; dual-core
boards also clear boot-state pages. `flash-chiperase` erases the whole chip.
`flash-recover` explicitly permits erasing protected memory, including settings
and security keys. `flash-app` writes only the application; `reset` uses probe-rs.
Nordic `mergehex` remains a build-time packaging tool, not a flashing backend.

## Marker control board

`marker-control-board` targets **nRF52833_xxAA**: one Cortex-M4 application and
one USB DFU bootloader, with no network core or staged bootloader. Source is
ported from [marker-control-board](https://github.com/odysseyarm/marker-control-board/tree/9c0779478329d84e9bfaafd6b11119c7a6ee3dea).
GPIO mapping and PCAL6408A current selection match that revision.

Cold start, reset, DFU entry and fault handlers force all four IR outputs low.
Cold start stays off until a complete debounced power-button click. Later clicks
toggle outputs. With no USB power, switching off enters Nordic System OFF;
button wake turns on only after release. USB-powered off state keeps DFU runtime
available. An initially held button on cold start cannot turn markers on.

Build without writing hardware:

```sh
cargo xtask firmware marker-control-board check
cargo xtask firmware marker-control-board build
cargo xtask firmware marker-control-board dist-bootloader
```

`firmware/marker-control-board/build/merged.hex` combines factory bootloader and
application. Initial migration from upstream firmware requires SWD:
`cargo xtask firmware marker-control-board flash`. This writes the board and
clears its boot-state page. Confirm ownership before flashing. `flash-app`
preserves bootloader but clears swap state; use only with the matching memory map.

Later application updates use `build/app.bin` through Odyssey Desktop **0.7.3+**
or `dfu-util -d 1915:5223 -a 0 -D firmware/marker-control-board/build/app.bin`.
Desktop sends runtime detach; for physical recovery hold the power button
(P0_11 low) while powering/resetting with USB attached. Missing or invalid
application vectors also leave the board in DFU. Application USB PID is `0x5213`;
DFU PID is `0x5223`, with the same FICR-derived serial in both modes.
Only `APP` alternate 0 is exposed; bootloader replacement requires SWD.
DFU requests persist in flash; incomplete downloads remain in DFU across resets.

[`memory/memory-marker-control-board.kdl`](memory/memory-marker-control-board.kdl)
defines bootloader `0x00000..0x10000`, state `0x10000..0x11000`,
application `0x11000..0x48000` (220 KiB), and DFU `0x48000..0x80000`
(224 KiB, including scratch). Interrupted swaps resume. A trial image is
confirmed only after outputs and current-driver initialization succeed;
unconfirmed images revert on reset. Vector checks validate address ranges and
Thumb entry, not image authenticity or integrity. DFU images are unsigned.

Release tags, compatibility floors, channel promotion and immutable published
hashes follow [firmware manifest instructions](manifests/README.md#marker-control-board).

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

Legacy tracking accepts control requests over USB and BLE independently of the
current report transport. Replies stay on the requesting transport; USB detach
or suspend drops USB replies instead of rerouting them over BLE. Partial USB
bulk requests survive BLE preemption, including multi-packet requests.

Modern ATS Lite DFU uses USB PID `0x5221` instead of application PID `0x5211`.
Desktop discovers both modes and lists the bootloader for firmware recovery.
A DFU request persists in flash, so resetting power does not necessarily return
the device to tracking. Modern `lite1-bootloader` waits 60 seconds for the first
download, then clears an abandoned request and boots the confirmed application.
This requires valid stored boot state, no pending trial boot or net-core recovery,
and an aligned RAM stack pointer plus a Thumb reset vector inside the active bank.
These checks do not authenticate or checksum the application image.
Any download attempt disables the timeout for the entire session, including after
an abort, USB reset, or transfer error. Recovery without a confirmed application
remains in DFU. Legacy bootloaders retain their existing behavior.
The timeout requires installing the updated bootloader; existing release 4 units
still need their firmware update completed through Desktop.
The modern bootloader release profile uses size optimization to fit its flash region.

Desktop treats a USB reset during the final DFU status poll as expected only after
the device acknowledges the zero-length download terminator and advertises
self-detach without manifestation tolerance. Linux can report this reset as
`Fault` ("hardware fault or protocol violation"). Transfer errors before that
acknowledgement and device-reported DFU errors still fail the update.
100% transfer progress does not verify installation: Desktop checks the reported
bootloader release, application version, and net-core release before accepting
their respective update steps.

Legacy ATSlite retains its fixed dual-bank flash layout. Its target configuration
uses `-Z location-detail=none` to omit runtime panic source paths while retaining
DWARF debug information for probe-rs. Build with the pinned nightly and
`cargo xtask firmware legacy-atslite build-app-a` / `build-app-b`, which rebuild the
standard library with size optimization; no bank boundaries or BLE features are changed.

Modern ATSlite keeps its object-mode packet queues in static storage. Built as
locals they inflate `main`'s poll frame, and spawning `ble_task` (whose future
embassy builds on the stack before moving it into its pool) then overflows the
stack into static RAM. Linker RAM usage excludes stack space; inspect prologue
`sub sp` sizes as well as static RAM usage when changing large buffers.
Object-mode queues use thread-mode mutexes and must only be accessed from the
application executor.

Both ATSlite applications arm the Cortex-M33 stack limit (MSPLIM) at the end of
static RAM in `__pre_init` (`atslite-common/src/nrf5340_init.rs`). A stack
overflow raises a UsageFault at the instruction that crosses the limit, which
probe-rs reports as a HardFault with `CFSR.STKOF` set, instead of corrupting statics.

Without a probe, a panic or hard fault resets the device immediately instead of
waiting for the 60 s watchdog. The handlers in `atslite-common/src/crash_record.rs`
keep a record in `.uninit` RAM across that reset, and the next boot logs
`previous boot crashed: <Panic|Fault> pc=… lr=… cfsr=…`. Panics record no
location (lite1 lacks the flash for it). With probe-rs attached, its default
hard-fault catch halts first, so the backtrace and panic message are still shown.

The calibration library requires native OpenCV. The Nix development shell supplies
OpenCV and clang include paths; Windows ChArUco detection uses
[OpenCV 4.9.0](https://github.com/opencv/opencv/releases/download/4.9.0/opencv-4.9.0-windows.exe).
Both applications offer explicit simulator mode when built with the `sim` feature
and launched with `--sim` (modern) or `--sim-legacy` (PAJ7025 reboot lifecycle).
Images are explicitly synthetic, not sensor observations. The legacy profile
exposes native NF/WF images without object samples, requires reboot after Stop,
and reconnects with a new USB endpoint; modern simulation remains reboot-free.