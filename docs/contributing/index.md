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

The default shell includes `cargo-release` for package-version and dependency
updates. Release patches and generated firmware manifests go through pull
requests: review, passing `host`, `rustfmt` and `tracking` checks, then squash
merge. `master` refuses direct pushes, force pushes and deletion, with no
bypass actors. Zero required approvals lets maintainers merge their own PRs
after reviewing the patch. Checks must be current with the base branch.
Publish only from the merged `master` commit; a pre-squash PR head is not a
release commit. See [firmware releases](../../firmware/manifests/README.md).

Host, format, tracking, FFI and UniFFI checks run only for suites a change can
affect. `.github/actions/impact` compares a pull request or merge group with its
base and selects suites from each crate's Cargo path-dependency closure plus
listed non-Cargo inputs, such as firmware manifests, Nix files and the
toolchain pin. Pushes to master, scheduled runs, manual runs on master and any
selector failure run every suite. A skipped suite reports success, so required
checks never wait: `host`, `rustfmt`, `tracking`, `FFI CI result` and
`UniFFI CI result`. `merge_group` is a trigger of every one of those workflows;
a merge queue must not be required for `master` before they all carry it.

Vision-tool simulation attaches to `odyssey_sim::UNIT_UUID`; USB presence and
reconnect generation come from that unit's snapshot, not global simulator state.
Check Host CI's optional simulator features, including simulator-control test targets:

```sh
cargo check --locked --offline -p odyssey-vision-tool --features sim --all-targets
cargo clippy --locked --offline -p odyssey-desktop --features sim --bin odyssey-sim-control --tests
```

### Rust lint and API policy

Run the pinned toolchain's checks from the repository root:

```sh
cargo --offline lint
cargo --offline lint-bins
nix build .#checks.x86_64-linux.xtask-lint .#checks.x86_64-linux.xtask-lint-bins
nix build .#checks.x86_64-linux.sdk-ffi-lint .#checks.x86_64-linux.sdk-uniffi-lint
```

`--offline` precedes `lint`: arguments after that alias reach Clippy's compiler
flags, not Cargo. Host, xtask and native SDK Clippy checks fail on warnings in CI.
Excluded SDK and firmware packages carry their own lint tables; root workspace
inheritance does not cross those boundaries.

The policy follows Microsoft's
[static-verification recommendations](https://microsoft.github.io/rust-guidelines/guidelines/universal/#M-STATIC-VERIFICATION)
and applicable [Google Rust guidance](https://google.github.io/styleguide/rustguide.html).
Pedantic checks and selected restriction/nursery checks are enabled; entire
restriction/nursery groups are not. Numeric casts, mathematical names, protocol
flag structs, line counts, configurable hashers and blanket `must_use` suggestions
have explicit exceptions. Crates.io metadata and duplicate transitive dependency
versions do not gate these bundled applications and SDKs. Chrome/Fuchsia-specific
layout, runtime and tooling rules do not apply.

Cargo's `unused_dependencies` and `unused_workspace_dependencies` are errors,
including during Clippy. They inspect selected packages and target configurations;
`dev-dependencies` are not covered by Cargo's unused-dependency lint. Use the pinned
toolchain rather than adding the obsolete `-Zcargo-lints` flag.

API availability uses `Since` rustdoc metadata:
published SDK APIs name their SDK release; internal Rust APIs name the first
Desktop release containing them, never an unpublished `0.0.0` crate version.
Items present in the earliest available tagged release identify that evidence
limit; current-only items say `Unreleased`. Check symbol history across file
moves rather than dating APIs from their current path.

Protocol variants also record a `Wire baseline`: the protocol counter in their
first tagged Desktop snapshot, not a minimum-support guarantee. Explicit minimum
device, mux and dongle-control contracts remain documented where established.
Rust API availability and peer wire compatibility are separate axes; comments
never replace negotiation gates or change encoding. Old independent endpoint
counters were reset during version unification and are not comparable.

Postcard compatibility is checked with `postcard-schema-ng` against
`crates/odyssey-protocol/tests/fixtures/postcard-compatibility.json`. This baseline
records the established wire contract at gate introduction, not schemas recovered
from every historical firmware image. Existing variant slots, field order, payload
types and bounds stay fixed; enums may append variants, including nested enums.
New variants still need peer-version or capability gates before transmission.
CBOR and packed legacy codecs retain their separate frozen-wire tests and proofs.

```sh
nix build .#checks.x86_64-linux.postcard-compatibility
cargo test --offline -p odyssey-protocol --features schema --test postcard_compatibility
UPDATE_POSTCARD_COMPATIBILITY=1 cargo test --offline -p odyssey-protocol --features schema --test postcard_compatibility
```

The last command explicitly updates the baseline for a published contract; it is
disabled in CI. Do not refresh it to hide an incompatible change. Schema derives
follow Serde conversion types, transparent wrappers and byte-array encoding.
Nalgebra field overrides describe its actual tuple encoding. Unsupported Serde
layout overrides fail schema compilation rather than silently using Rust layout.
Host CI checks host and firmware feature configurations. Firmware omits unused
image-response decoders by rejecting payloads while retaining enum slots; never
use `serde(skip)` or `skip_deserializing` to retire a published wire variant.
The same Nix check runs schema-enabled host Clippy with warnings treated as errors.

Pull requests and merge groups select this check from protocol crate dependencies,
schema fixtures, and shared Cargo, Nix and check configuration. Unrelated host-only
changes skip it. Main-branch pushes, events without a comparison base, and selector
failures still run every suite.

### Binary cache

`https://cache.odysseyarm.com` (Cloudflare R2) is the project's Nix binary
cache. CI trusts `odysseyarm-1` and the dedicated `odysseyarm-workstation-1`
signer (`.github/actions/setup-nix`). Host CI pushes the host dependency layer
(`.#host.cargoArtifacts`, built with the private-source stubs), xtask and
successful proof results on trusted runs. Firmware release workflows push
firmware derivations, checks and packaging closures; FFI and UniFFI workflows
push SDK derivations and checks on trusted runs. Desktop releases push
`desktop-nixos`.

Public builds upload full Nix build closures, including build-time dependency
outputs. The upload action rejects any closure containing the locked
`odyssey-cv` source. Host application and tracking derivations that compile its
real sources are never uploaded. Pull requests never push. Uploads need
`NIX_SIGNING_KEY`, `R2_ACCESS_KEY_ID` and `R2_SECRET_ACCESS_KEY`.

Check store selection, queue recovery, payload rejection and real local pushes:

```sh
nix shell --inputs-from . nixpkgs#python3 nixpkgs#zstd nixpkgs#jq nixpkgs#jujutsu --command bash .github/actions/push-cache/test.sh
```

Builds on a trusted workstation can populate the same cache. `publicCache` in
`nix/default.nix` is the whole list of what may leave the machine: `closures`
(firmware, SDK, xtask and the host dependency layer) are uploaded with their
build dependencies; `outputs` (proof results and `desktop-nixos`) are uploaded
alone, with their runtime closure. From the repository root:

```sh
bash .github/actions/push-cache/local.sh            # upload what is built
bash .github/actions/push-cache/local.sh --dry-run  # list the store paths
```

Approvals persist in `${XDG_STATE_HOME:-$HOME/.local/state}/odyssey-cache`.
Changing checkout does not replace earlier approvals. Completed builds are
recorded separately by derivation and output path; records do not grant upload
permission. Unknown builds stay local until an exact revision approves them.
Only valid, already-built outputs enter the upload queue. Never-built outputs
are skipped. A recorded completed output that was collected before queuing
instead blocks upload and requires rebuilding; the uploader builds nothing.

```sh
bash .github/actions/push-cache/local.sh --register 'git+file:///path/to/odyssey?rev=COMMIT'
bash .github/actions/push-cache/local.sh --flush
bash .github/actions/push-cache/local.sh --push --bookmark BRANCH
```

The push entrypoint asks `jj git push --dry-run` what would move, registers
every outgoing revision, flushes pending uploads, then pushes. Upload failure
blocks the push. Configure the local `jj push` alias to use this entrypoint;
plain `jj git push` bypasses it. A cache miss is not an instruction to build:
commit first, build the relevant checks/packages, then push.

The queue uses SQLite transactions and GC roots for pending output and
approved build closures. Failed uploads keep those roots; successful uploads
acknowledge the batch and release them. `--flush` needs no working-tree
evaluation, so retry remains possible after changing checkout.

It needs Nix, jj, Bash, Python 3.11+, jq and zstd,
`NIX_SIGNING_KEY_FILE` naming a dedicated signing key whose public key is in
CI's trusted keys, and bucket-scoped Cloudflare R2 credentials in the
S3-compatible AWS form (`AWS_PROFILE` or
`AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`). Keep both secrets outside the
repository with mode `600`.

For automatic uploads, configure nix-daemon's `post-build-hook` to write
`DRV_PATH` and `OUT_PATHS` as atomic records under a root-owned persistent
`/var/lib/nix-post-build/events` directory. The hook must never upload, run
checkout code, access credentials or fail the build. A user path unit watches
that directory; a user service settles build bursts, runs the uploader, and
repeats if more builds finished during upload. Start it at login as well, and
retry failures with backoff (30 seconds up to 15 minutes on this workstation).
Root permissions and credentials stay separate.

Before signing, the guard checks revision-bound private sources, retained
derivation provenance and payload contents, including nested archives. GC'd
deriver metadata is not rebuilt. Matching private Rust source bytes are
rejected even after copying/renaming or compression. Public build closures
and empty proof markers need no private-source fetch; other outputs require
available private-source fingerprints.
Malformed archive fixtures inside declared public build graphs are scanned
as raw data; private-consuming outputs fail closed on archive decode errors.
This is not arbitrary-secret detection: edited source and unrelated secrets
still require review before approving a new output family. Compiled private
code in the approved stripped desktop binary is intentional.

CI substitutes a result only when its derivation matches exactly: sources,
lockfiles, toolchains and target system. Every derivation here takes the
filtered source tree, so any tracked Rust change gives new cache keys, and
firmware images also embed the Git revision, so they match CI only when built
from a clean committed tree. Build after committing if CI should reuse the
result.

A trusted Nix signer can attest any store output, not just proofs. Protect its
private key accordingly; mathematical purity does not authenticate a builder.

### Desktop release types

Desktop publishes Stable from `odyssey-desktop-vX.Y.Z` tags. Scheduled runs
publish Nightly twice daily; `workflow_dispatch` publishes one on demand. Nightly
versions use the next patch and include UTC timestamp plus source commit, such as
`0.7.6-nightly.20260619.t020000.gabcdef`. GitHub marks these releases as
prereleases. Desktop has no self-updater; users choose releases and install them
manually.

The app reports full SemVer. Windows Installer accepts only three numeric
version fields, so MSI upgrade checks use the numeric `X.Y.Z` core; firmware
compatibility checks compare full SemVer, keeping a Nightly below matching
Stable floor.

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
