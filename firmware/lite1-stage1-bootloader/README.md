# First-Stage Bootloader for ATSlite (nRF5340)

This is the first-stage bootloader. It implements the A/B DirectXIP (eXecute-In-Place) boot logic for the ATSlite board.

## Architecture Overview

The ATSlite boots through a two-stage bootloader architecture in A/B DirectXIP mode:

```
┌─────────────────────────────────────────────────────────┐
│ Flash Memory Layout (Application Core)                  │
├─────────────────────────────────────────────────────────┤
│ 0x00000000 - 0x00001FFF │ Stage 1 Bootloader (8K)       │
│ 0x00002000 - 0x00005FFF │ Stage 2 Bootloader (16K)      │
│ 0x00006000 - 0x00006FFF │ Bootloader State (4K)         │
│ 0x00007000 - 0x0005BFFF │ Application Flash (340K)      │
│ 0x0005C000 - 0x000B1FFF │ DFU Slot (344K)               │
│ 0x000B2000 - 0x000B8FFF │ Settings (28K)                │
└─────────────────────────────────────────────────────────┘
```

## What is A/B DirectXIP?

A/B DirectXIP is two features put together. DirectXIP (eXecute-In-Place) means applications run directly from flash, with no copy to RAM or to another slot. A/B slots means there are two firmware slots, and each can boot independently.

### The Two Slots:
- Slot A (0x00007000, 340K) is the primary application slot.
- Slot B (0x0005C000, 344K) is the update/alternate slot.

### How It Works:
1. Firmware is downloaded to the inactive slot. It has to be built for that slot's address.
2. Boot state is updated to try the new slot.
3. The device resets and boots from the new slot.
4. If the app confirms boot, the update was successful and the new slot becomes active.
5. If the app doesn't confirm, the bootloader rolls back to the previous slot automatically after 1 failed boot.

### Benefits:
Boot is fast. There is nothing to copy, so the bootloader validates and jumps. Updates are safe because new firmware that fails gets rolled back automatically, and since the previous firmware stays intact during an update the device is always bootable. Flash wear stays low with no swap operations.

### Trade-offs:
Two builds are required, because firmware must be built for both slot addresses. Flash usage is larger as well. Both slots need space.

## Boot Process

The first-stage bootloader does the following on every boot:

1. Read boot state from flash (0x00006000). It records which slot is active, whether a pending update is waiting to be tried, and how many boot attempts there have been.
2. Try the pending slot, if it is different from the active one. This is a trial boot from the pending slot, and it increments the boot counter.
3. Check the boot counter. If boot_counter >= 2, roll back to the other slot. This prevents infinite boot loops.
4. Try the active slot. This is the normal boot, from the last confirmed working slot.
5. Fall back to the other slot. The alternate slot is tried if the active one fails.
6. Fall back to Stage 2. The second-stage bootloader boots for recovery/DFU.

## Boot State Tracking

The bootloader maintains state in the BOOTLOADER_STATE partition (0x00006000):

```rust
struct BootState {
    magic: u32,          // 0xB00710AD
    active_slot: u8,     // Last confirmed working slot (0 or 1)
    pending_slot: u8,    // Slot to try on next boot
    boot_counter: u8,    // Incremented on boot, cleared on confirm
    _reserved: u8,
}
```

### Boot Counter Rollback:
- `boot_counter == 0`: fresh boot or confirmed boot
- `boot_counter == 1`: first boot attempt (normal)
- `boot_counter >= 2`: the app failed to confirm, so the bootloader rolls back

## Image Validation

Validation starts with the magic number (`0x77c2_95f3`), which is compatible with embassy-boot. Images marked `BOOT_FLAG_BAD` are skipped. The vector table has to be valid too: stack pointer in RAM, thumb bit on the reset vector.

## Application Integration

Your application must call `BootConfirmation::confirm_boot()` after successful initialization:

```rust
use atslite_boot_api::BootConfirmation;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_nrf::init(Default::default());
    
    // Initialize your app...
    
    // Confirm boot is successful (prevents rollback)
    BootConfirmation::confirm_boot(p.NVMC).unwrap();
    
    // Rest of your app...
}
```

### Firmware Update Process:

```rust
// 1. Get the update slot address
let update_addr = BootConfirmation::get_update_slot_address();

// 2. Write new firmware to update slot (built for that address!)
write_firmware_to_flash(update_addr, new_firmware_data);

// 3. Mark update slot for boot
BootConfirmation::mark_other_slot_for_boot(nvmc).unwrap();

// 4. Reset device
cortex_m::peripheral::SCB::sys_reset();
```

## Building Firmware for Both Slots

Every firmware release needs two builds:

```bash
# Build for Slot A (0x00007000)
SLOT_ADDR=0x00007000 cargo build --release
mv target/release/app app-v1.0.0-slotA.bin

# Build for Slot B (0x0005C000)  
SLOT_ADDR=0x0005C000 cargo build --release
mv target/release/app app-v1.0.0-slotB.bin
```

Your linker script should use `$SLOT_ADDR` to set the correct flash origin.

## Building

From the repository root:

```bash
cargo xtask firmware lite1 build-stage1
```

## Flashing

Flash to address 0x00000000:

```bash
probe-rs download --chip nRF5340_xxAA --format Bin firmware/lite1-stage1-bootloader/target/thumbv8m.main-none-eabihf/release/lite1-stage1-bootloader 0x00000000
```

Or use:
```bash
cargo flash --release --chip nRF5340_xxAA
```

## Network Core Updates

DirectXIP mode rules out the traditional swap mechanism for NetCore updates. The application firmware is responsible for updating NetCore. Updates are transferred via IPC (Inter-Process Communication) using shared memory regions, and the application can write directly to NetCore flash or use RAM boot.

See the Zephyr MCUboot documentation for reference on network core IPC update mechanisms.

## Size Constraints

The first-stage bootloader must fit in 8KB (0x2000 bytes). It gets there today with LTO enabled (`lto = 'fat'`), size optimization (`opt-level = 'z'`) and minimal dependencies. No USB. No DFU protocol.

## See Also

- `firmware/lite1-bootloader/` is the second-stage bootloader, with the DFU functionality.
- `firmware/memory/memory-lite1.kdl` is the complete memory layout specification.
