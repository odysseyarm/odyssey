# Firmware Version Implementation Guide

## Current Status

The odyssey_desktop home page now displays connected devices with their UUID and transport type. A firmware version field is shown as "Not available" because the protocol does not yet support reading firmware versions from devices.

## What's Needed

To implement full firmware version support, the following changes are required across multiple repositories:

### 1. Extend the Protodongers Protocol

**Repository:** https://github.com/odysseyarm/protodonge-rs

**File:** `src/lib.rs`

Add FirmwareVersion to the PropKind and Props enums:

```rust
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PropKind {
    Uuid = 0,
    ProductId = 1,
    FirmwareVersion = 2,  // ADD THIS
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Props {
    Uuid([u8; 6]),
    ProductId(u16),
    FirmwareVersion(u32),  // ADD THIS - could be semver encoded as u32 or separate fields
}
```

Alternative version encoding options:
- `FirmwareVersion([u8; 3])` for major.minor.patch
- `FirmwareVersion { major: u8, minor: u8, patch: u8 }`
- `FirmwareVersion(u32)` where bits encode major/minor/patch

### 2. Update Device Firmware to Expose Version

**Repositories:**
- `D:\ody\donger\legacy\vm-fw` (ATS device firmware)
- `D:\ody\donger\dongle-fw` (Mux/dongle firmware)

Both firmwares need to:
1. Define their version (likely already done in build system)
2. Implement handler for PropKind::FirmwareVersion reads
3. Return their version in the Props::FirmwareVersion format

**Example location in dongle firmware:**
The dongle already has version support for the mux protocol:
- File: `D:\ody\donger\dongle-fw\src\main.rs` line 120
- Shows `const VERSION: Version` and `const USB_MUX_VERSION: UsbMuxVersion`

Need to expose this via the protodongers device control protocol.

### 3. Update odyssey_hub_server to Read Versions

**File:** `odyssey_hub_server/src/device_tasks.rs`

Add version reading alongside UUID reading (around lines 250, 412, 573):

```rust
// After reading UUID:
let firmware_version = match vm_device.read_prop(protodongers::PropKind::FirmwareVersion).await {
    Ok(protodongers::Props::FirmwareVersion(version)) => Some(version),
    Ok(other) => {
        warn!("Expected FirmwareVersion prop, got {:?}", other);
        None
    }
    Err(e) => {
        warn!("Failed to read firmware version from device: {}", e);
        None
    }
};
```

### 4. Extend Device Metadata

Two options:

**Option A: Extend Device struct (requires uniffi changes)**

**File:** `odyssey_hub_common/src/device.rs`

```rust
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Device {
    pub uuid: [u8; 6],
    pub transport: Transport,
    pub firmware_version: Option<u32>,  // ADD THIS
    pub product_id: Option<u16>,        // ADD THIS
}
```

Note: This requires updating all uniffi bindings and might break FFI compatibility.

**Option B: Create separate DeviceInfo struct (recommended)**

**File:** `odyssey_hub_common/src/device.rs`

```rust
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub device: Device,
    pub firmware_version: Option<u32>,
    pub product_id: Option<u16>,
}
```

Then update HubContext to store `Slab<DeviceInfo>` instead of `Slab<Device>`.

### 5. Add gRPC Service Methods

**File:** `odyssey_hub_server_interface/proto/service_interface.proto`

Add optional fields to device information:

```proto
message DeviceInfo {
  Device device = 1;
  optional uint32 firmware_version = 2;
  optional uint32 product_id = 3;
}

// Modify existing RPCs to return DeviceInfo instead of Device
rpc GetDeviceList(EmptyRequest) returns (DeviceListReply) {}

message DeviceListReply {
  repeated DeviceInfo devices = 1;  // Changed from repeated Device
}
```

### 6. Update Desktop UI

**File:** `odyssey_desktop/src/views/home.rs`

Once device info includes firmware version, update the UI:

```rust
div {
    class: "flex items-center gap-2 text-xs",
    span {
        class: "text-gray-500 dark:text-gray-400",
        "Firmware:"
    }
    if let Some(version) = device.firmware_version {
        span {
            class: "text-gray-700 dark:text-gray-200 font-mono",
            "{format_version(version)}"  // e.g., "1.2.3"
        }
    } else {
        span {
            class: "text-gray-400 dark:text-gray-500 italic",
            "Not available"
        }
    }
}
```

### 7. Add Version Comparison & Mismatch Warning

Define expected versions:

```rust
const EXPECTED_ATSLITE_VERSION: u32 = encode_version(1, 2, 3);
const EXPECTED_DONGLE_VERSION: u32 = encode_version(2, 0, 1);

fn encode_version(major: u8, minor: u8, patch: u8) -> u32 {
    ((major as u32) << 16) | ((minor as u32) << 8) | (patch as u32)
}

fn check_version_mismatch(device: &DeviceInfo) -> bool {
    if let Some(version) = device.firmware_version {
        let expected = match device.device.product_id {
            Some(0x5210) => EXPECTED_ATSLITE_VERSION,
            Some(0x5211) => EXPECTED_DONGLE_VERSION,
            _ => return false,
        };
        version != expected
    } else {
        false
    }
}
```

Display warning in UI:

```rust
if check_version_mismatch(&device) {
    div {
        class: "flex items-center gap-2 text-xs text-yellow-600 dark:text-yellow-400",
        span { "⚠" }
        span { "Firmware mismatch - update recommended" }
    }
}
```

## Implementation Order

1. **Protodongers protocol** - Add FirmwareVersion to PropKind/Props enums
2. **Device firmware** - Expose version via protodongers protocol
3. **Hub server** - Read version from devices during connection
4. **Common types** - Extend Device or create DeviceInfo struct
5. **gRPC protocol** - Add version fields to service interface
6. **Desktop UI** - Display version and mismatch warnings

## Testing Plan

1. Flash updated firmware to test devices
2. Connect device via USB and BLE (mux)
3. Verify version appears in desktop app home page
4. Test with mismatched firmware version to verify warning displays
5. Test with device that doesn't support version (should show "Not available")

## DFU Integration (Future Work)

Once versions are visible, DFU can be integrated:

1. Add "Update Firmware" button next to devices with version mismatches
2. Bundle expected firmware binaries in desktop app
3. Use `dfu-nusb` crate (already in dependencies) to perform update
4. Show progress bar during update
5. Reconnect and verify new version after update

**File:** `odyssey_desktop/Cargo.toml` already includes:
```toml
dfu-nusb = "0.1.1"
```
