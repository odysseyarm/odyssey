# Firmware manifests and releases

Hosts (Odyssey Desktop) learn what firmware exists from one JSON manifest per
channel and download the images from GitHub releases.

| File | Who follows it | How it changes |
|---|---|---|
| `releases.alpha.json` | Users who chose Alpha in Settings | `just gen-release` adds an entry |
| `releases.stable.json` | Everyone else (the default) | `promote.sh` copies an entry from alpha |

Hosts parse a manifest whole or not at all, and take the **first** entry they can
install, so entries are newest-first and a malformed entry hides every update.
`cargo test -p odyssey-hub-server --lib firmware` parses both files.

## Components and tags

Each image is released under its own tag. A manifest entry for an application
release pins the exact component releases it was tested with.

| Component | Tag | Version is | Assets |
|---|---|---|---|
| Application | `<device>-v<semver>` | Cargo version | `app-a.bin`, `app-b.bin`, `net-core.bin` |
| Stage2 bootloader | `<device>-bootloader-v<N>` | counter `N` in `bootloader/src/main.rs` | `stage2-bootloader.{bin,hex,elf}` |

`<device>` is the manifest device key: `atslite1` or `legacy-atslite1`.

The bootloader counter is the `release!` literal in its `main.rs`. It is
published as `bcdDevice = 0xF000 | N` and as the DFU label `BOOTLOADER N`, and
recorded in the manifest, all from that one number. Raise it whenever the image
changes. Do not reuse a number.

## Releasing a bootloader

1. Raise the counter in `bootloader/src/main.rs` and merge to `master`.
2. Publish it, either way:
   - **Locally:** from the board directory, `just release-bootloader`
     (`DRY_RUN=1` runs the checks and stops). It builds, refuses a dirty tree,
     a commit not on `origin/master`, or an existing tag, then creates the
     release as a draft and publishes it last.
   - **CI:** push the tag `<device>-bootloader-v<N>`. The
     *Bootloader Release* workflow checks the tag against the source, builds with
     the same `just dist-bootloader` and publishes. It does nothing if a release
     for the tag already exists, so the two routes cannot overwrite each other.
3. Add it to a firmware release (below). `gen-release` hashes the **published**
   asset, not a local build, because a build made elsewhere is not bit-identical
   and the manifest must describe the bytes devices download.

Do not delete a published bootloader release; manifest entries point at it.

## Releasing application firmware

From the board directory:

```
just build
just gen-release <requires_bootloader> <min_desktop>
```

- `requires_bootloader`: the lowest bootloader release this application can run
  on, `0` for none. The host refuses to install it unless the device's
  bootloader, after any update in the same session, reaches it. It must not be
  above the bootloader being published.
- `min_desktop`: the oldest Odyssey Desktop this firmware is required and tested
  with. It is not automatically the desktop shipping alongside; coupling a
  hotfix to a desktop upgrade it does not need strands users. Older desktops
  are told a newer firmware exists and where to get the desktop.

Neither has a default. Leaving one out would silently mean "any", so the recipe
and the manifest parser both refuse.

`gen-release` also refuses a bootloader image that changed without a higher
counter, a counter reused for a different image, and an image published under
two counters.

Then tag and publish the application release, and promote it once it has run on
alpha units:

```
firmware/manifests/promote.sh <device> <version>
```

`promote.sh` refuses to promote a version that is not newer than stable's
current newest for the device.

## Schema rules

- Hosts that predate a field ignore it. Adding a field never stops them parsing,
  but they also do not act on it: they still take the newest entry.
- A field new hosts must not default (`requires_bootloader`, `min_desktop`) is
  required in the host's parser, and every existing entry is backfilled.
- A change old hosts must not act on at all needs a bump of the manifest's
  top-level `version`, which hosts built after that check refuse to read.
