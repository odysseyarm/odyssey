# Device List Subscription Feature

## Overview
This feature allows the host to subscribe to automatic device list updates. When subscribed, the dongle firmware sends `DevicesSnapshot` messages whenever devices connect or disconnect.

## Implementation Details

### 1. Subscription State Management
- **Location**: `D:\ody\donger\dongle-fw\src\main.rs`
- **Flag**: `pub static DEVICE_LIST_SUBSCRIBED: AtomicBool`
- Thread-safe atomic boolean for tracking subscription state
- Initialized to `false` (not subscribed)

### 2. Message Handlers
- **Location**: `D:\ody\donger\dongle-fw\src\main.rs` in `handle_usb_rx()` function

#### Subscribe Handler
```rust
MuxMsg::SubscribeDeviceList => {
    info!("Subscribing to device list changes");
    DEVICE_LIST_SUBSCRIBED.store(true, Ordering::Relaxed);
    // Send initial snapshot
    let devices = active_connections.get_all().await;
    let response = MuxMsg::DevicesSnapshot(devices);
    HOST_RESPONSES.send(response).await;
}
```
- Sets subscription flag to `true`
- Immediately sends current device list as initial snapshot

#### Unsubscribe Handler
```rust
MuxMsg::UnsubscribeDeviceList => {
    info!("Unsubscribing from device list changes");
    DEVICE_LIST_SUBSCRIBED.store(false, Ordering::Relaxed);
}
```
- Sets subscription flag to `false`
- Stops automatic notifications

### 3. Automatic Notifications
- **Location**: `D:\ody\donger\dongle-fw\src\ble\central.rs` in `ActiveConnections` impl

#### On Device Connect
Modified `ActiveConnections::add()` to:
1. Add device to active connections
2. If subscription is active and add was successful, send DevicesSnapshot
3. Uses `try_send` to avoid blocking

#### On Device Disconnect
Modified `ActiveConnections::remove()` to:
1. Remove device from active connections
2. If subscription is active, send DevicesSnapshot
3. Uses `try_send` to avoid blocking

### 4. Thread Safety Considerations
- **AtomicBool**: Lock-free flag checking via `Ordering::Relaxed`
- **Lock Release**: Explicitly drops connection lock before sending notifications
- **Non-blocking Send**: Uses `try_send()` to avoid blocking if channel is full

## Usage Example

### Host Side (Python/Rust)
```python
# Subscribe to device list changes
send_message(MuxMsg.SubscribeDeviceList)
# ... receive initial DevicesSnapshot ...

# Wait for devices to connect/disconnect
# ... automatically receive DevicesSnapshot on each change ...

# Unsubscribe when done
send_message(MuxMsg.UnsubscribeDeviceList)
```

## Testing
1. Flash the modified firmware to the dongle
2. Connect host application
3. Send `MuxMsg::SubscribeDeviceList`
4. Verify initial `DevicesSnapshot` is received
5. Power on a BLE device and verify `DevicesSnapshot` is sent
6. Power off the device and verify another `DevicesSnapshot` is sent
7. Send `MuxMsg::UnsubscribeDeviceList`
8. Verify no more automatic snapshots are sent

## Benefits
- **Real-time Updates**: Host is notified immediately when devices connect/disconnect
- **Efficient**: No polling required, reduces USB traffic
- **Optional**: Host can choose whether to subscribe or use polling
- **Robust**: Uses lock-free atomics and non-blocking sends

## Files Modified
1. `D:\ody\donger\dongle-fw\src\main.rs` - Added subscription flag and message handlers
2. `D:\ody\donger\dongle-fw\src\ble\central.rs` - Added notification logic to add/remove methods

## Build Status
✅ Code compiles successfully with `cargo check`
✅ No new warnings introduced
✅ All changes are backward compatible (subscription is opt-in)
