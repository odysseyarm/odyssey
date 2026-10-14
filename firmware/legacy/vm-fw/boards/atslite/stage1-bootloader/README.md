# First-Stage Bootloader for ATSlite (nRF5340)

This is the first-stage bootloader that implements **A/B DirectXIP** (eXecute-In-Place) boot logic for the ATSlite board.

## Architecture Overview

The ATSlite uses a **two-stage bootloader architecture** with A/B DirectXIP mode:

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

**A/B DirectXIP** combines two powerful features:

1. **DirectXIP (eXecute-In-Place)**: Applications run **directly from flash** without copying to RAM or another slot
2. **A/B Slots**: Two independent firmware slots that can each boot independently

### The Two Slots:
- **Slot A** (0x00007000, 340K) - Primary application slot
- **Slot B** (0x0005C000, 344K) - Update/alternate slot

### How It Works:
1. Firmware is downloaded to the **inactive** slot (built for that slot's address)
2. Boot state is updated to try the new slot
3. Device resets and boots from the new slot
4. **If the app confirms boot** → update successful, new slot becomes active
5. **If the app doesn't confirm** → automatic rollback to previous slot after 1 failed boot

### Benefits:
- **Fast boot** - no copying, just validate and jump
- **Safe updates** - automatic rollback if new firmware fails
- **Always bootable** - previous firmware stays intact during update
- **Low flash wear** - no swap operations

### Trade-offs:
- **Two builds required** - firmware must be built for both slot addresses
- **Larger flash usage** - need space for both slots

## Boot Process

The first-stage bootloader performs the following on every boot:

1. **Read boot state** from flash (0x00006000)
   - Which slot is active?
   - Is there a pending update to try?
   - How many boot attempts?

2. **Try pending slot** (if different from active)
   - Boot from pending slot for trial
   - Increment boot counter
   
3. **Check boot counter**
   - If boot_counter >= 2: **ROLLBACK** to other slot
   - Prevents infinite boot loops
   
4. **Try active slot** (normal boot)
   - Boot from last confirmed working slot
   
5. **Fallback to other slot**
   - Try the alternate slot if active fails
   
6. **Fallback to Stage 2**
   - Boot second-stage bootloader for recovery/DFU

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
- `boot_counter == 0`: Fresh boot or confirmed boot
- `boot_counter == 1`: First boot attempt (normal)
- `boot_counter >= 2`: **Failed to confirm → ROLLBACK**

## Image Validation

Images are validated by checking:
- **Magic number** (`0x77c2_95f3`) - Compatible with embassy-boot
- **Not marked bad** - Images marked `BOOT_FLAG_BAD` are skipped
- **Vector table validity** - Stack pointer in RAM, reset vector has thumb bit

## Application Integration

Your application **MUST** call `BootConfirmation::confirm_boot()` after successful initialization:

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

You need **two builds** of every firmware release:

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

```bash
cd boards/atslite/stage1-bootloader
cargo build --release
```

## Flashing

Flash to address 0x00000000:

```bash
probe-rs download --chip nRF5340_xxAA --format Bin target/thumbv8m.main-none-eabihf/release/atslite-stage1-bootloader 0x00000000
```

Or use:
```bash
cargo flash --release --chip nRF5340_xxAA
```

## Network Core Updates

**Important**: DirectXIP mode means NetCore updates cannot use the traditional swap mechanism. Instead:

1. The **application firmware** is responsible for updating NetCore
2. Updates are transferred via **IPC** (Inter-Process Communication) using shared memory regions
3. The application can write directly to NetCore flash or use RAM boot

See the Zephyr MCUboot documentation for reference on network core IPC update mechanisms.

## Size Constraints

The first-stage bootloader **must fit in 8KB** (0x2000 bytes). Current optimizations:
- LTO enabled (`lto = 'fat'`)
- Size optimization (`opt-level = 'z'`)
- Minimal dependencies (no USB, no DFU protocol)

## See Also

- `boards/atslite/bootloader/` - Second-stage bootloader (DFU functionality)
- `memory/memory-atslite.kdl` - Complete memory layout specification
