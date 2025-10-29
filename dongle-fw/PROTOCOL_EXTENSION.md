# HubMsg Protocol Extension for Device Management

This document proposes additions to the HubMsg protocol in protodonge-rs for managing bonded devices.

## Proposed New Messages

Add these variants to `HubMsg` enum in `D:\ody\protodonge-rs\src\hub.rs`:

```rust
pub enum HubMsg {
    // ... existing variants ...
    
    /// Add a device UUID to the bond list (up to 7 devices)
    AddDevice(Uuid),
    
    /// Remove a device UUID from the bond list
    RemoveDevice(Uuid),
    
    /// Response after adding device
    AddDeviceResponse(Result<(), DeviceManagementError>),
    
    /// Response after removing device
    RemoveDeviceResponse(Result<(), DeviceManagementError>),
}

#[repr(C)]
#[cfg_attr(feature = "pyo3", pyo3::pyclass(get_all))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub enum DeviceManagementError {
    /// Device list is full (already 7 devices)
    ListFull,
    
    /// Device UUID already in list
    AlreadyExists,
    
    /// Device UUID not found
    NotFound,
    
    /// Storage error
    StorageError,
}
```

## Usage

### Adding a Device

```python
# Python example with protodongers
from protodongers import HubMsg, Uuid

# Add device with specific UUID
uuid = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06]
msg = HubMsg.AddDevice(uuid)
dongle.send(msg)

# Wait for response
response = dongle.receive()
if isinstance(response, HubMsg.AddDeviceResponse):
    if response.is_ok():
        print(f"Device {uuid} added successfully")
    else:
        print(f"Failed to add device: {response.error()}")
```

### Removing a Device

```python
# Remove device
msg = HubMsg.RemoveDevice(uuid)
dongle.send(msg)

# Wait for response
response = dongle.receive()
if isinstance(response, HubMsg.RemoveDeviceResponse):
    if response.is_ok():
        print(f"Device {uuid} removed successfully")
```

### Listing Devices

```python
# Request current device list
msg = HubMsg.RequestDevices()
dongle.send(msg)

# Receive snapshot
response = dongle.receive()
if isinstance(response, HubMsg.DevicesSnapshot):
    devices = response.devices
    print(f"Configured devices ({len(devices)}/7):")
    for uuid in devices:
        print(f"  - {uuid:02x}")
```

## Implementation Notes

1. Device UUIDs are stored in persistent flash storage using `sequential-storage`
2. The dongle will automatically attempt to connect to all configured devices on boot
3. Maximum 7 devices supported (defined by `MAX_DEVICES` constant)
4. Changes are persisted immediately to survive power cycles
