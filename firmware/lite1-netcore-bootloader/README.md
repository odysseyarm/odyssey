# Network Core Bootloader for ATSlite (nRF5340)

This bootloader runs on the network core. It receives firmware updates via IPC from the application core's second-stage bootloader.

## Architecture

The application core's bootloader starts at every boot. This one does not. On a normal boot the app core boots, then starts the network core application directly. The network core bootloader only runs in DFU mode, where the app core's second-stage bootloader drives it:

1. It receives network core firmware via USB DFU.
2. It starts the network core bootloader.
3. It transfers the firmware via IPC to the network core bootloader.
4. The network core bootloader writes to flash and boots the new firmware.

## Why This Design?

The nRF5340 has separate cores that communicate via IPC, and the application core controls when the network core starts. A DFU therefore begins on the app core. Its bootloader enters DFU mode over USB, and when network core firmware arrives it starts the net core bootloader, which stays in IPC listening mode while the app core sends firmware chunks via shared memory (ICMSG_RX/ICMSG_TX). The net core bootloader writes them to flash. Then it reboots.

## IPC Communication

The bootloader uses the same shared memory regions as BLE, which are not in use while a bootloader runs:

- ICMSG_RX (0x20070000, 8K) receives firmware from the app core.
- ICMSG_TX (0x20078000, 8K) sends acknowledgments to the app core.

See `firmware/atslite-common/src/ble/host/mod.rs` for IPC usage example.

## Memory Layout

```
Network Core Flash (256K @ 0x01000000):
├── 0x01000000 - 0x01001FFF │ Net Core Bootloader (8K)
├── 0x01002000 - 0x0101DFFF │ Net Core Application (120K)
└── 0x0101E000 - 0x01039FFF │ Net Core DFU Slot (120K)
```

## Building

From the repository root:

```bash
cargo xtask firmware lite1 build-netcore-bootloader
```

## Flashing

Flash to network core at 0x01000000:

```bash
probe-rs download --chip nRF5340_xxAA_net --format Bin firmware/lite1-netcore-bootloader/target/thumbv8m.main-none-eabi/release/lite1-netcore-bootloader 0x01000000
```

## Boot Flow

1. Check the DFU slot (0x0101E000). If it is valid and marked, boot it.
2. Check the App slot (0x01002000). If it is valid, boot it.
3. Otherwise, wait for an IPC firmware update from the app core.

## Implementation Status

Basic boot selection logic and image validation are done. IPC firmware reception is still a placeholder that needs a full implementation, and NVMC flash writing needs implementing as well.

## See Also

- `firmware/lite1-bootloader/` is the app core second-stage bootloader, which controls net core boot.
- `firmware/atslite-common/src/ble/host/mod.rs` has an IPC usage example.
- `firmware/memory/memory-lite1.kdl` is the memory layout.
