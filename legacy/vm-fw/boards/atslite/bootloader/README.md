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

## Features

- USB DFU 1.1 compatible
- Automatic WinUSB driver assignment (Windows)
- Image validation with magic numbers and CRC
- DirectXIP boot support via stage 1 integration

## See Also

- `boards/atslite/stage1-bootloader/` - First-stage bootloader with DirectXIP logic
- `memory/memory-atslite.kdl` - Complete memory layout specification
