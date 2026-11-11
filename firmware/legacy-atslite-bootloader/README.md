# Second-Stage Bootloader for ATSlite (nRF5340)

This is the second-stage bootloader for the ATSlite board. It provides DFU (Device Firmware Update) over USB, using `embassy-boot` to interact with flash and `embassy-usb-dfu` for the USB DFU protocol.

## Two-Stage Boot Architecture

This bootloader is stage 2 of a two-stage system. Flash is divided like this:

- Stage 1 (8K @ 0x00000000) validates and boots images using DirectXIP.
- Stage 2 (16K @ 0x00002000) is this bootloader. It handles USB DFU updates.
- Application (340K @ 0x00007000) is the main application firmware.
- DFU Slot (344K @ 0x0005C000) is the download slot for new firmware.

## DirectXIP Mode

The system uses DirectXIP (eXecute-In-Place). Images run directly from their flash slot, with no swap or copy operations between slots, which is where it departs from a traditional bootloader. The first-stage bootloader handles boot selection. This stage mostly manages DFU download and validation.

## Usage

### Building

From the repository root:

```bash
cargo xtask firmware legacy-atslite build-bootloader
```

### Flashing

Flash to address 0x00002000:

```bash
probe-rs download --chip nRF5340_xxAA --format Bin firmware/legacy-atslite-bootloader/target/thumbv8m.main-none-eabihf/release/legacy-atslite-bootloader 0x00002000
```

Or use:
```bash
cargo flash --release --chip nRF5340_xxAA
```

The first-stage bootloader has to be flashed as well, at 0x00000000.

## DFU Mode

The bootloader enters DFU mode when the application calls `embassy_boot::FirmwareUpdater::mark_dfu()` and resets. It also ends up there when no valid application image exists to boot.

Once in DFU mode it exposes a USB DFU interface, downloads new firmware to the DFU slot and marks the image for boot. Then it resets, and stage 1 boots the new firmware.

## Network Core Updates

Network core updates are the application's job. DirectXIP constraints keep them out of the bootloader. The application downloads NetCore firmware through its normal update mechanism and transfers the image over IPC to the network core, which can then be updated via RAM boot or a direct flash write.

See the main application documentation for NetCore update procedures.

## Watchdog Requirements

Stage 2 starts a hardware watchdog (WDT0) with a **60-second timeout**. Once started it **cannot be stopped**, and it keeps running after the bootloader hands over to the application.

Stage 1 could have started it. Starting it in stage2 means the watchdog configuration can be changed through a stage2 bootloader update, which leaves more room for future changes.

### Application Requirements

Your application **must** feed the watchdog, or the system resets. The stage2 bootloader already started it, so what you need is a handle to the running watchdog.

Option 1 is to get a WatchdogHandle from the already-running watchdog:

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

Option 2 is a background task. This is the recommended one:

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

The stage2 bootloader feeds the watchdog on its own while it runs. Your app takes over after boot.

## Features

The bootloader is USB DFU 1.1 compatible and exposes 3 alt interfaces:

- Alt 0: Inactive App Bank (for app updates)
- Alt 1: Network Core (for netcore updates)
- Alt 2: Bootloader (for bootloader self-updates via swap)

On Windows the WinUSB driver is assigned automatically. Images are validated with magic numbers and CRC. The bootloader updates itself by swap with rollback protection, and it calls `mark_booted()` on successful boot to prevent rollback.

## See Also

- `firmware/legacy-atslite-stage1-bootloader/` holds the first-stage bootloader with the DirectXIP logic.
- `firmware/memory/memory-atslite.kdl` is the complete memory layout specification.
