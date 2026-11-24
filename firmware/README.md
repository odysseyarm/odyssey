## Board flashing

`cargo xtask firmware <board> flash` runs `probe-rs download` with read-back
verification and a reset. Probe-rs picks the flash algorithm for each nRF5340
core from the addresses in the merged HEX. A normal flash erases the sectors it
touches, and on dual-core boards it also clears the boot-state pages.

The other commands go further or do less. `flash-chiperase` erases the whole
chip. `flash-recover` explicitly permits erasing protected memory, which
includes settings and security keys. `flash-app` writes only the application,
and `reset` goes through probe-rs. Nordic `mergehex` is still used at build time
to package images. It flashes nothing.

## Marker control board

`marker-control-board` targets **nRF52833_xxAA**. That means one Cortex-M4
application and one USB DFU bootloader, with no network core and no staged
bootloader. The source is ported from
[marker-control-board](https://github.com/odysseyarm/marker-control-board/tree/9c0779478329d84e9bfaafd6b11119c7a6ee3dea),
and the GPIO mapping and PCAL6408A current selection match that revision.

Cold start, reset, DFU entry and the fault handlers all force the four IR
outputs low. Cold start selects **Auto** with outputs off. A complete debounced
click of the power button cycles **Auto → On → Off → Auto**, and a button that
is already held at boot is ignored until it is released. Manual On overrides
automatic demand. Going back to Auto discards old demand, so the outputs stay
off until a fresh lease arrives. The radio stays available in Auto and Off, even
without USB power, because these modes do not enter System OFF. USB DFU runtime
stays available whenever USB is attached.

The application and the bootloader each reserve 512 bytes for Microsoft OS
descriptors, since WinUSB IDs and interface GUIDs appear at both the device and
the DFU-function level. A 256-byte buffer overflows during USB setup. The board
then sits in a reset loop and never starts advertising.

The board advertises **Odyssey Markers** with a stable FICR-derived static-random
address and accepts one GATT central. The shared wire contract is
[`odyssey_protocol::marker_control`](../crates/odyssey-protocol/src/marker_control.rs):

| GATT attribute | UUID | Value |
|---|---|---|
| Service | `9b7a0001-4f4e-4f44-5953-53455941524d` | Marker control |
| Demand | `9b7a0002-4f4e-4f44-5953-53455941524d` | Acknowledged one-byte write: `0` clears, `1` renews |
| Mode | `9b7a0003-4f4e-4f44-5953-53455941524d` | One byte: Auto `0`, Off `1`, On `2` |

In Auto, active demand enables the outputs for **6 seconds**. The hub renews
every **2 seconds**, and a disconnect, an explicit clear or lease expiry
switches the outputs off. The hub sends demand only for registered boards whose
assigned screen has an active event subscriber. Unassigned boards stay off.
Several boards may share one of the six screen IDs.

Control writes require an encrypted link from the single approved owner bond.
An unencrypted write gets `INSUFFICIENT_ENCRYPTION`, and an encrypted write from
anyone else gets `INSUFFICIENT_AUTHORISATION`. To enroll, hold the power button
for **3 seconds** (after boot, starting from a released button). That opens a
30-second enrollment window. The host pairs inside it through BlueZ, WinRT or
the Odyssey dongle, and enrollment succeeds only when the board acknowledges the
OFF write, which happens after the owner bond is committed to flash and read
back. A new owner replaces the old one only after that commit.

Hold the button for **10 seconds** to clear the owner bond (flash and the BLE
host's table), then open a fresh enrollment window. The LED turns red from the
3 second mark until the clear, flashes white four times when it is done, then
shows blue. Releasing before 10 seconds keeps the bond; the enrollment window
from the 3 second mark stays open.

Locked boards remain discoverable. The advertisement carries 128-bit service
data (AD type `0x21`) for the marker service UUID: version `1`, followed by one
byte of remaining enrollment seconds (`0..=30`, where zero means closed). The
full **Odyssey Markers** name is in the scan response. While the board is
advertising, that service data is refreshed whenever the window physically
changes and at each countdown boundary. Desktop enrollment goes by the deadline
the selected radio reports. Discovering a board is not enough.

The RGB status LED (PCAL6408A P5 red, P6 green, P7 blue, per schematic 117-27606
rev B) shows **blue** while the enrollment window is open and **red** while the button
is held toward a bond clear. Otherwise it is
**green** while outputs are on from hub demand, **yellow** in manual On,
**magenta** in manual Off, and dark in idle Auto. The expander has no PWM, so
colours are on/off mixes. If markers stay lit or dark while the hub shows Idle,
look at the LED: a short button click that left it yellow or magenta is why.

Pairing is LE Secure Connections **Just Works**. The link is encrypted and
bonded, but an active attacker who races the 30-second window is not kept out.
The lease limits stale demand after a host failure. A host keeps its stale OS
bond when a board is enrolled elsewhere, so forget the board in the OS before
re-enrolling. Desktop connects to a board only while its screen has demand or
during enrollment, and it disconnects after acknowledging OFF.

Build without writing hardware:

```sh
cargo xtask firmware marker-control-board check
cargo xtask firmware marker-control-board build
cargo xtask firmware marker-control-board dist-bootloader
```

`firmware/marker-control-board/build/merged.hex` combines the factory bootloader
and the application. The first migration from upstream firmware requires SWD:
`cargo xtask firmware marker-control-board flash`. This writes the board and
clears its boot-state page, so confirm ownership before flashing. `flash-app`
preserves the bootloader but clears swap state. Use it only with the matching
memory map.

Application updates use `build/app.bin` through Desktop with signed-DFU support.
For `dfu-util`, first create a signed `.dfu` envelope as described in
[signed releases](manifests/README.md#signed-releases), then download it with
`dfu-util -d 1915:5223 -a 0 -D app.dfu`. Older bootloaders still take raw `.bin`.
Desktop sends a runtime detach. For physical recovery, hold the power button
(P0_11 low) while powering or resetting with USB attached. Missing or invalid
application vectors also leave the board in DFU. The application USB PID is
`0x5213` and the DFU PID is `0x5223`, with the same FICR-derived serial in both
modes. Only `APP` alternate 0 is exposed, which means replacing the bootloader
requires SWD. DFU requests persist in flash, and an incomplete download leaves
the board in DFU across resets.

[`memory/memory-marker-control-board.kdl`](memory/memory-marker-control-board.kdl)
defines bootloader `0x00000..0x0e000` (56 KiB), owner-bond pages
`0x0e000..0x10000`, state `0x10000..0x11000`,
application `0x11000..0x48000` (220 KiB), and DFU `0x48000..0x80000`
(224 KiB, including scratch). SWD programming must preserve `0x0e000..0x10000`.
A full-chip erase clears the owner and requires physical re-enrollment.
Interrupted swaps resume. A trial image is confirmed only after the outputs and
the current driver initialize successfully, and an unconfirmed image reverts on
reset. Vector checks validate address ranges and Thumb entry; they do not prove
authenticity. Bootloader 1.0.0 additionally requires a trusted release signature
and matching image hash before activating a USB DFU download.

Release tags, compatibility floors, channel promotion and immutable published
hashes follow [firmware manifest instructions](manifests/README.md#marker-control-board).

## Bluetooth OTA

Odyssey Desktop downloads the app and, for ATS Lite, net-core assets from the
firmware manifest, verifies their published size and SHA-256, then stages them
over an encrypted, owner-authorized BLE link. Marker control boards accept a
system Bluetooth controller or an Odyssey dongle. ATS Lite boards use an
Odyssey dongle or, on Linux, the computer's own Bluetooth adapter once it is
paired from Desktop ("Pair this computer"); the dongle relays packets and is
not an OTA receiver. The transfer library is shared by the marker and ATS Lite
applications.

ATS Lite firmware with device protocol 0.2.2 or later stores up to four bonds
and refuses a fifth host until one is removed. The slot choice is
[`slot-table`](slot-table/src/lib.rs), proved with Verus
(`nix build .#checks.x86_64-linux.slot-table-proofs`). The same firmware
advertises with a static random address: its uuid with the top two bits of the
last byte set (`odyssey_protocol::control::ble_address`). Linux only stores
bonds for a random address of that form. A unit whose uuid did not already have
those bits set changes address when it is updated, so a dongle bonded to it
before the update has to be paired again.

Modern ATS Lite, legacy ATS Lite and marker applications require trusted Ed25519
release signatures before activating OTA updates. Their 1.0.0 bootloaders also
authenticate USB DFU; see [signed releases](manifests/README.md#signed-releases).

Incomplete transfers are not activated. A complete image is read back and
checked before the application or boot state changes. ATS Lite's stage2
bootloader still performs the net-core install at reset; stage2 bootloader
updates themselves remain USB-only. Newly generated ATS Lite releases require
stage2 bootloader `1.0.0` or later. The direct-XIP journal cutover below still
requires a paired SWD installation; do not offer it as a standalone bootloader
update to released units. If a BLE update reports failure or the device enters
recovery, connect that same unit over USB and use Desktop's firmware update
flow; it verifies bootloader, application and net-core versions after updates.

After a net-core copy, the network bootloader finishes its state-page write
before reporting success, then waits for stage2 to stop it. The application
alone starts the radio and IPC; starting them before that handoff leaves stale
state on the first boot after an update.

An interrupted radio transfer is cleared before the next transfer starts.
Marker staging erases flash one page at a time so the first data write does
not wait for the whole slot to erase. Network-core copies include flash-word
padding; signed image lengths and hashes exclude that padding.

ATS Lite stage2 bootloaders retry failed network-core copies within their
existing attempt limit, then enter USB DFU. A fallback application bank becomes
the recorded active bank so the next application update retains trial-boot
rollback. Battery read failures retain the last published state, and skipped
samples remain included in the next fuel-gauge elapsed interval.

## Adaptive marker exposure

lite1 adjusts the sensor exposure (register 0x67) while tracking so marker spots
stay bright but unsaturated. The controller is `ExposureController` in
`odyssey_cv::exposure`, whose module docs describe the whole loop. It measures
only marker-like spots: round or small, at most 300 px, plus marker-sized
saturated spots. Windows and light strips are ignored. Exposure moves in small
steps, at most every 400 ms and within 8× either side of the stored setting,
because each write needs the sensor in idle mode and drops a frame. If no spot
is measurable for 1 s, exposure walks back toward the stored setting, so a
distractor cannot leave it too low to see the markers again. Saving settings
stores the user's exposure, not the adapted one.

While the hub is tracking, it sends the image positions of the spots it matched to
the marker pattern (vendor tag `0xfd`, `odyssey_protocol::marker_hint`) at up to
10 Hz. It sends nothing when fewer than four markers are matched, so there are
no hints while the hub is not tracking. For 300 ms after a hint, the firmware
measures only spots near those positions, which excludes round lamps and
reflections; without a live hint it uses the shape rules above. Positions are
on the sensor's report grid, 1/64 pixel. Older firmware ignores the tag. Legacy
ATS Lite does not adapt its exposure.

## Live image inspection and calibration

Calibration runs inside `odyssey-vision-tool`. `viewer` is now a reusable
calibration library and no longer a separate capture application. Open **Image /
camera calibration** (or the sensor settings' capture action), start the USB
preview, capture target views, then solve and export or apply the calibration.
Calibration uses the main window, and **Back to tracking** stops the preview. On
modern cameras tracking resumes immediately. On legacy PAJ7025 it still requires
the recovery described below. Mono results include RMS and per-view errors.
Stereo calibration consumes paired NF/WF observations. Solving uses native image
pixels, and exported and applied camera matrices are scaled into the selected
device's tracking coordinates.

In Odyssey desktop, open **Image / IR** on a USB-connected device to inspect the
native grayscale image, observed FPS and receive age. Image capture is
image-only, so neither application displays object diagnostics or centroid
overlays. Calibration target corners remain visible in vision-tool.

Both applications request **15 FPS** by default, adjustable from **1–15 FPS**.
That number is the requested rate. Nobody measured it as a sensor maximum.
Acquisition and bounded USB transfers can lower the observed rate. Firmware does
not clamp requests to 5 FPS or add a full frame interval after transfer time.

Modern ATSlite captures 320×240 NF images without rebooting. The sensor stays in
image mode throughout capture, and optical tracking pauses. Stop, a stream drop,
USB disconnect or a capture failure restores live sensor settings and object
mode. IMU and control traffic remain independent. Images are USB-only, and BLE
requests are rejected explicitly.

Modern capture is leased. Start grants 5 s, and the viewer renews every second
while it is still consuming frames. The hub client renews from the frame
stream's poll and vision-tool from its frame loop. The hub never renews on its
own. Without a same-generation renewal the firmware ends capture, restores
tracking and reports `LeaseExpired`, even with USB still attached, so a crashed
or hung viewer or hub cannot leave the camera in image mode. Older firmware
lacks the lease and must be updated. A firmware hang is outside the lease's
reach. Legacy ignores renewals and keeps its reboot recovery.

Legacy PAJ7025 ATSlite/VM uses the recovered sensor-master image entry procedure
and a Nordic SPIS receiver to acquire native **98×98 NF or WF** grayscale pixels
(up to 15 requested FPS). The trailing sensor transport bytes are not pixels. No
object samples are acquired during image capture. Image configuration is
temporary, and entering or stopping capture never flashes over the saved
tracking settings.

**Legacy capture requires reboot/power-cycle and reconnect to restore tracking.**
Both UIs warn before entry. **Stop**, closing the dialog, a capture failure and
USB disconnect all end pixel reception but leave the sensor in image mode. A
generation-bound Stop returns `RebootRequired`, and a stale Stop cannot stop the
current generation. Further capture and settings operations are refused until
recovery. The firmware never recreates a register SPI master or tries an
undocumented reverse register sequence while the sensor drives its SPI pins.

In vision-tool, choose **Stop preview & reboot legacy device**. In Odyssey
desktop, choose **Stop images & reboot legacy device** in **Image / IR**.
Closing the dialog keeps a recovery warning on the device row. Both use the
existing USB control reboot acknowledgement, with no DFU and no firmware
flashing involved. Wait for a fresh USB connection, because the acknowledgement
alone does not prove tracking has recovered. In vision-tool, reconnect and
reselect the same unit in configuration before applying or flashing calibration.
A successful connection and settings read unlocks those actions. NF/WF captures
and fits are kept for the same device UUID across reconnects, so you can go on
to collect the other camera after recovery.

If the control reboot is unavailable (VM hardware without a control endpoint is
one case), or the unit does not return to tracking, fully power-cycle it and
reconnect. Unplugging USB alone may not turn off a battery-powered ATSlite.
Saved settings return at boot, so reapply any unsaved sensor edits after
recovery. The original image firmware used this same reboot workflow. It does
not establish a safe reboot-free image-master → register-slave transition.

Legacy tracking accepts control requests over USB and BLE independently of the
current report transport. Replies stay on the requesting transport. On USB
detach or suspend, USB replies are dropped and never rerouted over BLE. Partial
USB bulk requests survive BLE preemption, including multi-packet requests.

Modern ATS Lite DFU uses USB PID `0x5221` where the application uses `0x5211`.
Desktop discovers both modes and lists the bootloader for firmware recovery.
A DFU request persists in flash, so resetting power does not necessarily return
the device to tracking. Modern `lite1-bootloader` waits 60 seconds for the first
download, then clears an abandoned request and boots the confirmed application.
This requires valid stored boot state, no pending trial boot or net-core
recovery, and an aligned RAM stack pointer plus a Thumb reset vector inside the
active bank. These checks do not authenticate or checksum the application image.
Any download attempt disables the timeout for the entire session, and that still
holds after an abort, a USB reset, or a transfer error. Recovery without a
confirmed application remains in DFU. Legacy bootloaders retain their existing
behavior. The timeout only exists once the updated bootloader is installed, so
existing release 4 units still need their firmware update completed through
Desktop. The modern bootloader release profile uses size optimization to fit its
flash region.

The ATS Lite direct-XIP development port uses `embassy-boot` from the fork's
`integration` branch, patched by `firmware/.cargo/config.toml`. Application
trials use a two-page journal at `0xF6000..0xF8000`; DFU requests and net-core
commands stay at `0xFA000`. Bank addresses and the three settings guard pages
are unchanged. An unconfirmed application gets one boot before rollback.
Application-bank writes are rejected while an update is pending or on trial.

This state format requires a paired bootloader/application installation.
Provision both through SWD and erase `0xF6000..0xF8000` when switching formats.
An empty journal prefers valid bank A, so install the matching new bank-A app
or erase that bank when provisioning bank B. Older app/bootloader combinations
are unsupported after this cutover. Do not publish these builds as standalone
updates for released firmware. Stage1 and net-core swap formats remain unchanged.

Desktop treats a USB reset during the final DFU status poll as expected only
after the device acknowledges the zero-length download terminator and advertises
self-detach without manifestation tolerance. Linux can report this reset as
`Fault` ("hardware fault or protocol violation"). Transfer errors before that
acknowledgement and device-reported DFU errors still fail the update. Reaching
100% transfer progress does not verify installation. Desktop checks the reported
bootloader release, application version, and net-core release before it accepts
each of those update steps.

Legacy ATSlite retains its fixed dual-bank flash layout. Its target
configuration uses `-Z location-detail=none` to omit runtime panic source paths
while keeping DWARF debug information for probe-rs. Build with the pinned
nightly and `cargo xtask firmware legacy-atslite build-app-a` / `build-app-b`,
which rebuild the standard library with size optimization. No bank boundaries or
BLE features are changed.
Both ATS Lite application and stage2 bootloader builds rebuild the standard
library with size optimization; stage2 also uses fat LTO and
`-C panic=immediate-abort`. A stage2 panic therefore traps without a message;
only the unexpected-interrupt handler still logs its IRQ number through defmt.
With CC312 Ed25519 and Oberon hashing linked, the modern stage2 leaves about
1 KiB of its 44 KiB region and the legacy one about 1.5 KiB, so measure before
adding code or log statements there.

Modern ATSlite keeps its object-mode packet queues in static storage. Built as
locals they inflate `main`'s poll frame, and spawning `ble_task` (whose future
embassy builds on the stack before moving it into its pool) then overflows the
stack into static RAM. Linker RAM usage excludes stack space, so when you change
large buffers, inspect prologue `sub sp` sizes as well as static RAM usage.
Object-mode queues use thread-mode mutexes and must only be accessed from the
application executor.

Both ATSlite applications arm the Cortex-M33 stack limit (MSPLIM) at the end of
static RAM in `__pre_init` (`atslite-common/src/nrf5340_init.rs`). A stack
overflow then raises a UsageFault at the instruction that crosses the limit, and
the statics are left uncorrupted. Probe-rs reports it as a HardFault with
`CFSR.STKOF` set.

Without a probe, a panic or hard fault resets the device immediately. It does
not wait for the 60 s watchdog. The handlers in
`atslite-common/src/crash_record.rs` keep a record in `.uninit` RAM across that
reset, and the next boot logs
`previous boot crashed: <Panic|Fault> pc=… lr=… cfsr=…`. Panics record no
location (lite1 lacks the flash for it). With probe-rs attached, its default
hard-fault catch halts first, so the backtrace and panic message are still
shown.

The calibration library requires native OpenCV. The Nix development shell
supplies OpenCV and clang include paths. Windows ChArUco detection uses
[OpenCV 4.9.0](https://github.com/opencv/opencv/releases/download/4.9.0/opencv-4.9.0-windows.exe).
Both applications offer an explicit simulator mode when built with the `sim`
feature and launched with `--sim` (modern) or `--sim-legacy` (PAJ7025 reboot
lifecycle). The images are explicitly synthetic. They are not sensor
observations. The legacy profile exposes native NF/WF images without object
samples, requires a reboot after Stop, and reconnects with a new USB endpoint.
Modern simulation remains reboot-free.

## Nix builds

`nix/firmware.nix` builds every board without rustup. There is one package per
firmware ELF (`firmware-<board>-<part>`, with `app-a`/`app-b` for the dual-bank
boards), plus packages that run the same `cargo xtask` conversion and packaging
on those ELFs: `firmware-<board>-images` (`build`),
`firmware-<board>-images-dual-bank`, `bootloader-dist-<board>`,
`netcore-dist-<board>` and `dongle-dist`. The release workflows build these.
xtask takes the ELFs from `ODYSSEY_PREBUILT` (a tree laid out like the checkout)
and does not run cargo, and `ODYSSEY_REPO` names the scratch checkout it writes
into. A Nix build cannot check for a clean tree.
`cargo xtask dongle check-clean` does that, so the dongle workflow runs it on
the real checkout first.

The git dependencies are fetched by hash from `nix/firmware-git-hashes.json`.
Run `nix/update-firmware-git-hashes.sh` after changing one.
`nix develop .#firmware` is the flashing and signing shell (probe-rs, nrfutil with
nrf5sdk-tools). `dongle-fw/fanstel-debug.pem` and `laird-dongle.pem` are tracked
private keys. They are the vendors' published DFU signing keys, and only
`dongle-dist` receives them.
