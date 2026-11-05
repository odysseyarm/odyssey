# Contributing

Documentation for people working on the Odyssey repositories: how the system
fits together, how to build and test it, and how releases are made.

- How tracking works inside the hub is documented in the private odyssey-cv
  repository, alongside the tracking code.

## Existing notes

These still live next to the code they describe:

- [Tracking regression tests](https://github.com/odysseyarm/odyssey/blob/master/TRACKING_REGRESSION.md)
- [Odyssey Desktop offline mode](https://github.com/odysseyarm/odyssey/blob/master/crates/odyssey-desktop/OFFLINE.md)
- [Autocalibration markers](https://github.com/odysseyarm/odyssey/blob/master/crates/odyssey-autocalib/MARKERS.md)
  and [calibration board](https://github.com/odysseyarm/odyssey/blob/master/crates/odyssey-autocalib/tools/BOARD.md)

## Screen calibration

Calibration saves belong to the wizard scope, not the overwrite confirmation
dialog: confirming closes that dialog before persistence completes. Keep the
accepted result and screen ID fixed for the duration of the save; a failed
write must leave the review available for retry.

The headless save regression uses isolated simulator storage and covers dialog
unmount, replacement confirmation, validation failure and retry:

```sh
cargo test -p odyssey-desktop --features sim \
  confirmed_overwrite_survives_dialog_unmount_and_errors_can_retry
```
