# Second-Stage Bootloader for ATSlite (nRF5340)

This is the **second-stage bootloader** that provides DFU (Device Firmware Update) functionality via USB for the ATSlite board. It uses `embassy-boot` to interact with flash and `embassy-usb-dfu` for the USB DFU protocol.

## Two-Stage Boot Architecture

This bootloader is **stage 2** of a two-stage system:

- **Stage 1** (8K @ 0x00000000): Validates and boots images using DirectXIP
- **Stage 2** (16K @ 0x00002000): This bootloader - handles USB DFU updates
- **Application** (340K @ 0x00007000): Main application firmware
- **DFU Slot** (344K @ 0x0005C000): Download slot for new firmware

## DirectXIP Mode

Unlike traditional bootloaders, this system uses **DirectXIP** (eXecute-In-Place):
- Images run directly from their flash slot
- No swap/copy operations between slots
- First-stage bootloader handles boot selection
- This stage primarily manages DFU download and validation

## Usage

### Building

```bash
cd boards/atslite/bootloader
cargo build --release
```

### Flashing

Flash to address 0x00002000:

```bash
probe-rs download --chip nRF5340_xxAA --format Bin target/thumbv8m.main-none-eabihf/release/atslite-bootloader 0x00002000
```

Or use:
```bash
cargo flash --release --chip nRF5340_xxAA
```

**Note**: Make sure the first-stage bootloader is also flashed at 0x00000000.

## DFU Mode

The bootloader enters DFU mode when:
1. The application calls `embassy_boot::FirmwareUpdater::mark_dfu()` and resets
2. There's no valid application image to boot

In DFU mode:
- Exposes a USB DFU interface
- Downloads new firmware to the DFU slot
- Marks the image for boot
- Resets, allowing stage 1 to boot the new firmware

## Network Core Updates

**Important**: Network core updates are handled by the **application**, not the bootloader, due to DirectXIP constraints:

- Application downloads NetCore firmware via its normal update mechanism
- Application transfers NetCore image via IPC to the network core
- Network core can be updated via RAM boot or direct flash write

See the main application documentation for NetCore update procedures.

## Watchdog Requirements

**CRITICAL**: Stage 2 bootloader starts a hardware watchdog (WDT0) with a **60-second timeout**. This watchdog **cannot be stopped** once started and persists after booting into the application.

**Why stage2 starts the watchdog**: Starting the watchdog in stage2 (instead of stage1) allows the watchdog configuration to be updated via stage2 bootloader updates, providing more flexibility for future changes.

### Application Requirements

Your application **MUST** feed the watchdog to prevent system resets. The watchdog was started by the stage2 bootloader and you need to obtain a handle to it.

**Option 1**: Get a WatchdogHandle from the already-running watchdog:

```rust
// The watchdog is already running (started by bootloader)
// We just need to get a handle to feed it
use embassy_nrf::wdt::{Watchdog, Config, SleepConfig, HaltConfig};

let mut wdt_config = Config::default();
wdt_config.timeout_ticks = 32768 * 60;
wdt_config.action_during_sleep = SleepConfig::RUN;
wdt_config.action_during_debug_halt = HaltConfig::PAUSE;

let (_wdt, [wdt_handle]) = Watchdog::try_new(p.WDT0, wdt_config)
    .expect("Watchdog already running");

// Feed it periodically
wdt_handle.pet();
```

**Option 2**: Use a background task (recommended):

```rust
#[embassy_executor::task]
async fn watchdog_feeder(mut wdt_handle: embassy_nrf::wdt::WatchdogHandle) {
    use embassy_time::{Duration, Timer};
    
    loop {
        wdt_handle.pet();
        Timer::after(Duration::from_secs(30)).await;
    }
}

// In your main function:
let (_wdt, [wdt_handle]) = Watchdog::try_new(p.WDT0, wdt_config)
    .expect("Watchdog already running");
spawner.spawn(watchdog_feeder(wdt_handle).expect("Failed to spawn watchdog feeder"));
```

**Note**: The stage2 bootloader automatically feeds the watchdog during operation. Your app continues this after boot.

## Features

- USB DFU 1.1 compatible with 3 alt interfaces:
  - Alt 0: Inactive App Bank (for app updates)
  - Alt 1: Network Core (for netcore updates)
  - Alt 2: Bootloader (for bootloader self-updates via swap)
- Automatic WinUSB driver assignment (Windows)
- Image validation with magic numbers and CRC
- Swap-based bootloader self-update with rollback protection
- Calls `mark_booted()` on successful boot to prevent rollback

## See Also

- `boards/atslite/stage1-bootloader/` - First-stage bootloader with DirectXIP logic
- `memory/memory-atslite.kdl` - Complete memory layout specification
