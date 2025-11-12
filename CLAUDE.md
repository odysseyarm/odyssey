# Odyssey Desktop Development Notes

## Fixed Issues

### Device Not Reconnecting After Power Cycle (VM or Dongle)
**Status**: ✅ FIXED

**Root Cause**: 
- Scan targets (list of BLE device addresses to connect to) were stored only in RAM
- When dongle rebooted, scan_targets list was empty
- Without any targets, dongle never tried to connect to anything
- Bonded device info existed in flash, but wasn't being used to populate scan targets on startup

**Solution Implemented**:
- Modified `BleManager::run()` in `dongle-fw/src/ble/central.rs`
- On startup, if no scan targets are configured, populate them from bonded devices
- This ensures bonded devices are automatically reconnected after dongle reboot

**Code Change**:
```rust
// If no scan targets configured, populate from bonded devices
if targets.is_empty() && !pairing {
    let bonds = self.settings.get_all_bonds().await;
    for bond_opt in bonds.slots.iter() {
        if let Some(bond) = bond_opt {
            let _ = self.settings.add_scan_target(bond.bd_addr).await;
        }
    }
    targets = self.settings.get_scan_targets().await;
}
```

**Files Modified**:
- `dongle-fw/src/ble/central.rs` - Added bond-based scan target population

## Architecture Notes

### Select Pattern Refactoring (Completed)
- ✅ dongle-fw/src/main.rs: Replaced select with 2 independent tasks using Mutex
- ✅ dongle-fw/src/ble/central.rs: Replaced select4 with 4 independent tasks  
- ✅ legacy/vm-fw/peripheral.rs: Split into 4 independent tasks
- ⏸️ dongle-fw/src/storage.rs: Kept nested select (unavoidable due to shared resource access)

Key improvement: Eliminated task cancellation patterns, ensuring no async operations are left in sketchy states.
