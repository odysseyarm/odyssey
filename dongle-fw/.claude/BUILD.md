# Building for Different Hardware Variants

This firmware supports two hardware configurations with different memory layouts:

## Building

### For Dongle (Default)
```bash
cargo build --release
# or simply
cargo build
```

### For DK
```bash
cargo build --release --config .cargo/config-dk.toml --features rtt
```

**Note**: The DK build uses the `rtt` feature to enable RTT logging instead of USB serial logging.

## Flashing

### Dongle
```bash
# Put dongle in bootloader mode first
nrfutil pkg generate --hw-version 52 --sd-req 0x00 --application-version 1 \
    --application target/thumbv7em-none-eabihf/release/dongle-fw \
    dongle-fw.zip

nrfutil dfu usb-serial -pkg dongle-fw.zip -p <PORT>
```

### DK
```bash
# Using probe-rs (recommended)
cargo run --release --config .cargo/config-dk.toml --features rtt

# Or using probe-rs directly
probe-rs run --chip nRF52840_xxAA target/thumbv7em-none-eabihf/release/dongle-fw

# View RTT logs (in a separate terminal)
probe-rs attach --chip nRF52840_xxAA
```

## Feature Flags

### `rtt`
Enables RTT (Real-Time Transfer) logging via `defmt-rtt` instead of USB serial logging. This is recommended for DK builds where you have a debugger attached.

**When to use:**
- DK builds with probe-rs or J-Link debugger
- Development and debugging scenarios

**When NOT to use:**
- Dongle builds (no debugger interface)
- Production builds

## Using in Code

The RTT feature is available as a cfg in your code:

```rust
#[cfg(feature = "rtt")]
fn rtt_specific() {
    // RTT-specific code (DK)
}

#[cfg(not(feature = "rtt"))]
fn usb_specific() {
    // USB serial logging code (dongle)
}
```

The board variant can be detected via the `BOARD` environment variable during build time (see `build.rs`).

## Troubleshooting

If you get an error about missing memory files:
- Make sure `memory-dongle.kdl` exists for dongle builds
- Make sure `memory-dk.kdl` exists for DK builds
- Check that `CARGO_CFG_BOARD` environment variable is set correctly
