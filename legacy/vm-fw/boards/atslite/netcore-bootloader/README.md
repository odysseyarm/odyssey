# Network Core Bootloader for ATSlite (nRF5340)

This bootloader runs on the **network core** and receives firmware updates via IPC from the application core's second-stage bootloader.

## Architecture

Unlike the application core, the network core bootloader is **NOT** automatically started at boot. Instead:

1. **Normal boot**: App core boots, then starts the network core application directly
2. **DFU mode**: App core's second-stage bootloader:
   - Receives network core firmware via USB DFU
   - **Starts the network core bootloader**
   - Transfers firmware via IPC to network core bootloader
   - Network core bootloader writes to flash and boots new firmware

## Why This Design?

The nRF5340 has separate cores that communicate via IPC. The application core controls when the network core starts. During DFU:

- App core bootloader enters DFU mode (USB)
- When network core firmware is received, app bootloader starts net core bootloader
- Net core bootloader stays in IPC listening mode
- App core sends firmware chunks via shared memory (ICMSG_RX/ICMSG_TX)
- Net core bootloader writes to flash and reboots

## IPC Communication

Uses the same shared memory regions as BLE (they're not in use during bootloader):

- **ICMSG_RX** (0x20070000, 8K): Receive firmware from app core
- **ICMSG_TX** (0x20078000, 8K): Send acknowledgments to app core

See `boards/atslite/src/ble/host/mod.rs` for IPC usage example.

## Memory Layout

```
Network Core Flash (256K @ 0x01000000):
├── 0x01000000 - 0x01001FFF │ Net Core Bootloader (8K)
├── 0x01002000 - 0x0101DFFF │ Net Core Application (120K)
└── 0x0101E000 - 0x01039FFF │ Net Core DFU Slot (120K)
```

## Building

```bash
cd boards/atslite/netcore-bootloader
cargo build --release
```

## Flashing

Flash to network core at 0x01000000:

```bash
probe-rs download --chip nRF5340_xxAA_net --format Bin target/thumbv8m.main-none-eabihf/release/atslite-netcore-bootloader 0x01000000
```

## Boot Flow

1. Check DFU slot (0x0101E000) - if valid and marked, boot it
2. Check App slot (0x01002000) - if valid, boot it  
3. Otherwise, wait for IPC firmware update from app core

## Implementation Status

- ✅ Basic boot selection logic
- ✅ Image validation
- ⚠️  IPC firmware reception (placeholder - needs full implementation)
- ⚠️  NVMC flash writing (needs implementation)

## See Also

- `boards/atslite/bootloader/` - App core second-stage bootloader (controls net core boot)
- `boards/atslite/src/ble/host/mod.rs` - IPC usage example
- `memory/memory-atslite.kdl` - Memory layout
