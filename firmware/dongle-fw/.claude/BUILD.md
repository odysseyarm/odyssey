# Building for Different Hardware Variants

This firmware supports two hardware configurations with different memory layouts.

## Quick Start

Use `just` commands (see [Justfile](../Justfile) for all available commands):

### For Dongle (Default)
```bash
# Build release firmware
just build-release

# Flash via DFU (put dongle in bootloader mode first)
just dfu-release

# Build and flash in one step
just flash-release
```

### For DK
```bash
# Build DK firmware with RTT logging
just build-dk

# Flash to DK via probe-rs
just flash-dk

# Run with RTT logs
just run-dk
```

## Manual Building

### For Dongle
```bash
cargo build --release
```

### For DK
```bash
cargo build --release --config .cargo/config-dk.toml --features rtt
```

**Note**: The DK build uses the `rtt` feature to enable RTT logging instead of USB serial logging.

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
