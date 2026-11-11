# Contributing

Documentation for people working on the Odyssey repositories: how the system
fits together, how to build and test it, and how releases are made.

- How tracking works inside the hub is documented in the private odyssey-cv
  repository, alongside the tracking code.

## Developer tools

The repository's `flake.lock` pins Nixpkgs, Crane and the Rust overlay. Enter the
default environment with `nix develop`; `direnv` users can run `direnv allow`
instead. No sibling Nix checkout is needed. `nix-shell` remains available through
the locked flake-compat entry point.

```sh
nix develop
cargo build --locked --offline --package odyssey-desktop
nix build .#host
nix build .#xtask
nix build .#checks.x86_64-linux.tracking-tests
```

Crane builds dependencies separately from application sources. Nix fetches pinned
sources before sandboxed Cargo builds run offline. Host builds need access to the
private `odyssey-cv` revision in `flake.lock`; its revision must match `Cargo.lock`.
Public SDK and firmware workspaces do not need that private source.

Specialized environments are `nix develop .#sdk`, `.#firmware` and `.#tracking`.
Firmware artifacts and SDK platform outputs are listed by `nix flake show`.
Release workflows consume those outputs; Windows WiX assembly and macOS builds
still need their respective operating systems.

Vision-tool simulation attaches to `odyssey_sim::UNIT_UUID`; USB presence and
reconnect generation come from that unit's snapshot, not global simulator state.
Check the optional feature used by Host CI:

```sh
cargo check --locked --offline -p odyssey-vision-tool --features sim --all-targets
```

### Binary cache

`https://cache.odysseyarm.com` (Cloudflare R2) is the project's Nix binary
cache, signed with `odysseyarm-1`. Every CI job reads from it
(`.github/actions/setup-nix`). Host CI pushes the host dependency layer
(`.#host.cargoArtifacts`, built with the private-source stubs) and xtask on
trusted master runs. Firmware release workflows push firmware derivations,
checks and packaging closures; FFI and UniFFI workflows push SDK derivations
and checks on trusted runs. Desktop releases push `desktop-nixos`.

Public builds upload full Nix build closures, including build-time dependency
outputs. The upload action rejects any closure containing the locked
`odyssey-cv` source. Host application and tracking derivations that compile its
real sources are never uploaded. Pull requests never push. Uploads need
`NIX_SIGNING_KEY`, `R2_ACCESS_KEY_ID` and `R2_SECRET_ACCESS_KEY`.

Run `.github/actions/push-cache/test.sh` to check build-only closure coverage
and private-source rejection locally.

### NixOS package

Desktop releases attach the pushed store path as
`odyssey-desktop-nixos-store-path.txt`.

To install it, add the cache to your NixOS configuration:

```nix
nix.settings.substituters = [ "https://cache.odysseyarm.com" ];
nix.settings.trusted-public-keys = [
  "odysseyarm-1:pQObqsDwZYUoo1CsL/NGvRPF+aAfKtDyZALe7CVyuI0="
];
```

then fetch the path named by the release and install it:

```sh
release=odyssey-desktop-v0.7.5  # choose a release tag that includes this asset
store_path=$(curl -fsSL "https://github.com/odysseyarm/odyssey/releases/download/$release/odyssey-desktop-nixos-store-path.txt")
nix copy --from https://cache.odysseyarm.com "$store_path"
nix profile install "$store_path"
```

Releases before this asset existed (0.7.4 and earlier) are not in the cache.

Run the pinned Dioxus CLI with `cargo bin dx` inside the project's development
shell; no global installation is needed. Its Subsecond tooling supports Rust
hot patching independently of Dioxus UI. The registry entry alone does not enable
GPUI's `hot-reload` feature.

## Desktop setup workspace

Setup shares the selected display and tracker with calibration and zeroing.
Calibration runs inside the main window; its measurement pattern opens on the
selected display. Leaving an unfinished calibration asks before discarding it.
Pairing returns to Setup when opened there. Device details own firmware progress,
acknowledged results and advanced sensor controls.

GPUI identifies displays by logical bounds without names. Workspace lookup
normalizes legacy named assignments in memory, keeping their bounds and leaving
files untouched. Current unnamed assignments take precedence; conflicting legacy
assignments stay unknown. Missing wizard metadata does not invalidate saved
calibration files.

Calibration entry defers the main-window update until its click callback returns.
Verify entry through the Calibrate screen button, not only the startup command:
updating the current window from its own callback is rejected by GPUI.

Zeroing keeps the target at the display center. Select a tracker, aim at the
target, press Z, verify the live crosshair, then Save zero. Shot capture and delay
controls live under Advanced. Applied and saved alignment are separate states;
leaving with an unsaved or unknown result requires confirmation. Save zero stores
the alignment in Odyssey for that tracker, not in tracker flash.

Accessories separates **Odyssey** boards from **Third-party** accessories and
always lists supported models: Marker Control Board, DryFireMag and MantisX.
Register a nearby marker board, then assign **Screen 1–6**; multiple boards may
share a screen. Unassigned boards receive no automatic illumination demand.
Third-party accessories retain tracker assignment instead of screen assignment.
Each accessory also has a radio: the system Bluetooth radio (default), a Linux
adapter, or an Odyssey USB dongle. Odyssey shows live and pending links per radio
and reports capacity only where a dongle states it; native capacity is unknown.
A fixed radio that is unplugged keeps its assignments and never falls back to
another radio. Enrolling a marker board needs its physical pairing window.

Nearby shows each Bluetooth address once, with a selector for the radios that
detected it. Radio cards wrap horizontally; **Details** expands each card
independently without hiding capacity or availability warnings.

Discoverable marker boards are not necessarily pairable. Release the power
button after boot, then hold it for **3 seconds** and wait for **Pairing window
open** with its countdown. Enrollment stays disabled for closed, expired or
unknown windows on the selected radio. Unknown status needs updated marker
firmware and, for Odyssey dongles, firmware **0.1.12+**. Only the selected
accessory shows enrollment progress; changing its radio does not change saved
ownership until enrollment succeeds.

A short press of the board's button cycles its mode Auto → On → Off, which can
leave markers lit while the row says Idle. **Reset to Auto** on a saved marker
row connects briefly on the board's saved radio, writes Mode = Auto with an
acknowledged GATT write and disconnects; the hub keeps no standing connection.
Dongles need firmware **0.1.13+** (mux protocol 0.4.0); older dongles report an
update-required error instead of sending the command.

The desktop's background event stream is passive. Zeroing owns a separate active
subscription for the selected display's saved, valid screen calibration. Changing
screens drops that subscription, clears tracking history and requests the new
screen; leaving Zero drops its request. Zero ignores tracking samples from other
screens. Independent client subscriptions keep their requested screens active
until their streams close. See [marker firmware behavior and BLE security limits](https://github.com/odysseyarm/odyssey/blob/master/firmware/README.md#marker-control-board).

## Existing notes

These still live next to the code they describe:

- [Tracking regression tests](https://github.com/odysseyarm/odyssey/blob/master/TRACKING_REGRESSION.md)
- [Odyssey Desktop offline mode](https://github.com/odysseyarm/odyssey/blob/master/crates/odyssey-desktop/OFFLINE.md)
- [Autocalibration markers](https://github.com/odysseyarm/odyssey/blob/master/crates/odyssey-autocalib/MARKERS.md)
  and [calibration board](https://github.com/odysseyarm/odyssey/blob/master/crates/odyssey-autocalib/tools/BOARD.md)

## Screen calibration

Calibration saves belong to the wizard task, not the confirmation UI. Confirming
replacement removes that UI before persistence completes. Keep the accepted
result and screen ID fixed for the duration of the save; a failed write must
leave the review available for retry.

The headless save regression uses isolated simulator storage and covers
replacement, remembered display settings and preservation after invalid input:

```sh
cargo test -p odyssey-desktop --features sim \
  saving_replaces_a_screen_and_a_rejected_calibration_keeps_it
```

Calibration text beside icons needs `flex_1().min_w_0()` on the text column
and `flex_shrink_0()` on the icon wrapper so paragraphs wrap within the card.
Check “How big is the screen?” at 600px and 760px window widths, including
the missing-display-size advice, setup hint and tape/card measurement prompt.
