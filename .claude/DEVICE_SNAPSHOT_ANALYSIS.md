# Device Snapshot Generation and Handling - Detailed Analysis

## Overview

This document provides a comprehensive analysis of where and how "device snapshot" is generated and sent in the dongle-fw project. The device snapshot is a list of currently connected BLE devices, transmitted to the USB host in response to a `RequestDevices` message.

**Note:** There is no "odyssey_hub_server" project in the donger codebase. The USB host receives the snapshots via the dongle-fw USB interface.

---

## High-Level Flow

```
Host (PC)                          Dongle-FW (nRF52840)
    |                                  |
    |---- RequestDevices (MuxMsg) ---->|
    |                                  |
    |                                  | (in handle_usb_rx)
    |                                  | 1. Receive RequestDevices
    |                                  | 2. Call active_connections.get_all()
    |                                  | 3. Build DevicesSnapshot
    |                                  | 4. Queue response to HOST_RESPONSES
    |                                  |
    |<-- DevicesSnapshot (MuxMsg) -----|
    |                                  | (from host_responses_tx_task)
    |                                  | Send via USB bulk IN endpoint
```

---

## Detailed Code Locations

### 1. RequestDevices Handling - `dongle-fw/src/main.rs`

**Function:** `handle_usb_rx()` (lines ~620-680)

```rust
async fn handle_usb_rx(
    data: &[u8],
    device_queues: &ble::central::DeviceQueues,
    active_connections: &ActiveConnections,
) -> Result<(), ()> {
    // Decode postcard-encoded MuxMsg
    let msg: MuxMsg = postcard::from_bytes(data).map_err(|e| {
        error!("Failed to deserialize USB packet ({} bytes): {:?}", data.len(), ...);
        ()
    })?;

    match msg {
        MuxMsg::RequestDevices => {
            let devices = active_connections.get_all().await;  // <-- SNAPSHOT BUILT HERE
            let response = MuxMsg::DevicesSnapshot(devices);
            HOST_RESPONSES.send(response).await;  // <-- QUEUED FOR TRANSMISSION
        }
        // ... other message types ...
    }

    Ok(())
}
```

**Key Points:**
- This function is spawned as part of the **usb_rx_task** (lines ~490-520)
- Receives raw USB data from the host over the bulk OUT endpoint
- Uses `postcard::from_bytes()` to deserialize the MuxMsg
- **Snapshot creation:** Calls `active_connections.get_all().await`
- **Transmission:** Sends the response to the static `HOST_RESPONSES` channel

---

### 2. Snapshot Generation - `dongle-fw/src/ble/central.rs`

**Structure:** `ActiveConnections` (lines ~160-220)

```rust
pub struct ActiveConnections {
    connections: AsyncMutex<ThreadModeRawMutex, heapless::LinearMap<Uuid, (), MAX_DEVICES>>,
}

impl ActiveConnections {
    pub const fn new() -> Self {
        Self {
            connections: AsyncMutex::new(heapless::LinearMap::new()),
        }
    }

    pub async fn get_all(&self) -> heapless::Vec<Uuid, MAX_DEVICES> {
        let map = self.connections.lock().await;
        let mut result = heapless::Vec::new();
        for (uuid, _) in map.iter() {
            let _ = result.push(*uuid);
        }
        result
    }
}
```

**Key Points:**
- `ActiveConnections` is a thread-safe map of currently connected device UUIDs
- Each UUID is a 6-byte BLE address (`[u8; 6]`)
- `get_all()` method iterates the map and collects all UUIDs into a `heapless::Vec`
- Maximum capacity is `MAX_DEVICES` (8 devices, from protodongers crate)
- Returns type: `heapless::Vec<Uuid, MAX_DEVICES>` which is serializable via postcard

**Global Instance:** `dongle-fw/src/ble/central.rs` (line ~155)

```rust
pub static ACTIVE_CONNECTIONS: StaticCell<ActiveConnections> = StaticCell::new();
```

Initialized in `main()` at line ~180 in `dongle-fw/src/main.rs`:

```rust
let active_connections = ACTIVE_CONNECTIONS.init(ActiveConnections::new());
```

---

### 3. Response Transmission - `dongle-fw/src/main.rs`

**HOST_RESPONSES Channel Definition** (lines ~21-23)

```rust
// Channel for sending host responses (RequestDevices, ReadVersion, etc.)
type HostResponseChannel = Channel<ThreadModeRawMutex, MuxMsg, 4>;
static HOST_RESPONSES: HostResponseChannel = Channel::new();
```

**Transmission Task:** `host_responses_tx_task()` (lines ~562-578)

```rust
// Sends host responses (RequestDevices, ReadVersion, etc.) to USB endpoint
async fn host_responses_tx_task(
    ep_mutex: &'static embassy_sync::mutex::Mutex<
        ThreadModeRawMutex,
        Option<embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>>,
    >,
) {
    let mut write_buf = [0u8; 256];

    loop {
        let response = HOST_RESPONSES.receive().await;  // <-- DEQUEUE SNAPSHOT

        let mut ep_opt = ep_mutex.lock().await;
        if let Some(ep) = ep_opt.as_mut() {
            if send_mux_msg(&response, &mut write_buf, ep).await.is_err() {
                warn!("USB TX: error sending host response");
                break;
            }
        }
    }
}
```

**Serialization:** `send_mux_msg()` (lines ~600-612)

```rust
async fn send_mux_msg(
    msg: &MuxMsg,
    write_buf: &mut [u8],
    ep_in: &mut embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>,
) -> Result<(), ()> {
    use embassy_usb::driver::EndpointIn;

    // Encode using postcard (standard for USB in ats_usb)
    let data = postcard::to_slice(msg, write_buf).map_err(|_| ())?;

    // Use write_transfer to handle large packets automatically
    EndpointIn::write_transfer(ep_in, data, true)
        .await
        .map_err(|_| ())?;
    Ok(())
}
```

**Key Points:**
- `HOST_RESPONSES` is a 4-slot channel (capacity for 4 pending responses)
- `host_responses_tx_task` is spawned within `usb_tx_task` (line ~540)
- Response is serialized using postcard encoding
- Sent via USB bulk IN endpoint using `write_transfer()`

---

### 4. Active Connection Management - `dongle-fw/src/ble/central.rs`

**When Connections are Added:** In `central_conn_task()` (line ~850)

```rust
// Register device queue for outgoing packets
let device_queue = match this.device_queues.register(&target.into_inner()).await {
    Some(queue) => queue,
    None => {
        error!("Failed to register device queue for {:02x}", target);
        return Err(());
    }
};

// Add to active connections
if !this.active_connections.add(&target.into_inner()).await {
    error!("Failed to add device to active connections");
    this.device_queues.unregister(&target.into_inner()).await;
    return Err(());
}
```

**When Connections are Removed:** In `central_conn_task()` (line ~908)

```rust
async fn central_conn_task<P: PacketPool>(
    conn: &trouble_host::prelude::Connection<'_, P>,
    active_connections: &ActiveConnections,
    device_queues: &DeviceQueues,
    _target: BdAddr,
) {
    loop {
        match conn.next().await {
            trouble_host::prelude::ConnectionEvent::Disconnected { reason } => {
                info!("Disconnected: {:?}", reason);
                active_connections.remove(&_target.into_inner()).await;  // <-- REMOVE
                device_queues.unregister(&_target.into_inner()).await;
                return;
            }
            // ... other events ...
        }
    }
}
```

---

## Control.rs Analysis

**File:** `dongle-fw/src/control.rs`

This file implements the **USB control plane** (EP0) for control messages like `StartPairing`, `ClearBonds`, etc., not the bulk data plane where `RequestDevices` is handled.

**Key Functions:**
- `try_send_event()` - Non-blocking send of control events to host
- `try_recv_event()` - Non-blocking receive of control events from host
- `recv_cmd()` - Async receive of commands from host

**Relationship to Device Snapshot:**
- `control.rs` handles control-plane messages (UsbMuxCtrlMsg) on EP0
- Device snapshot handling uses bulk endpoints and `MuxMsg` (not `UsbMuxCtrlMsg`)
- Control plane is separate from data/snapshot plane for reduced contention

---

## Data Structures and Types

### MuxMsg Enum

Defined in `protodongers` crate (external dependency):

```rust
pub enum MuxMsg {
    RequestDevices,
    SendTo(SendTo { dev: Uuid, pkt: Packet }),
    ReadVersion(),
    DevicesSnapshot(HVec<Uuid, MAX_DEVICES>),  // <-- SNAPSHOT TYPE
    DevicePacket(DevicePacket),
    ReadVersionResponse(Version),
}
```

### Uuid Type

From `dongle-fw/src/ble/central.rs` (line ~67):

```rust
pub type Uuid = [u8; 6];
```

The "Uuid" is actually a 6-byte BLE Bluetooth Device Address (BdAddr), not a standard UUID.

### DevicesSnapshot Response

The snapshot response is a vector of up to 8 device addresses:

```rust
MuxMsg::DevicesSnapshot(devices)
// where devices: heapless::Vec<[u8; 6], 8>
```

When serialized with postcard:
- Vector length (1-2 bytes)
- 6 bytes per device address
- Maximum payload: ~50 bytes for 8 devices

---

## USB Communication Stack

### Bulk Endpoints

- **OUT (Host → Device):** `ep_out` receives RequestDevices
  - Handled in `usb_rx_task()` → `handle_usb_rx()`

- **IN (Device → Host):** `ep_in` sends DevicesSnapshot
  - Handled in `usb_tx_task()` → `host_responses_tx_task()`

### Endpoint Configuration

From `dongle-fw/src/usb.rs`:
- Bulk OUT endpoint receives 512-byte packets
- Bulk IN endpoint sends up to 256-byte packets
- Both use postcard encoding

---

## Key Files Summary

| File | Responsible For | Key Functions |
|------|-----------------|----------------|
| **main.rs** | RequestDevices handling, response transmission | `handle_usb_rx()`, `host_responses_tx_task()`, `usb_rx_task()`, `usb_tx_task()` |
| **ble/central.rs** | Active connection tracking, snapshot building | `ActiveConnections::get_all()`, `ActiveConnections::add()`, `ActiveConnections::remove()` |
| **control.rs** | USB EP0 control plane (separate from snapshots) | `try_send_event()`, `recv_cmd()` |
| **ble/mod.rs** | BLE module exports, device packet channel | `DevicePacketChannel` type definition |
| **usb.rs** | USB device configuration and initialization | USB device setup |

---

## Sequence: From RequestDevices to DevicesSnapshot

1. **Host sends:** Postcard-encoded `MuxMsg::RequestDevices` on bulk OUT endpoint
2. **USB RX task** receives data in buffer (512 bytes max)
3. **handle_usb_rx()** decodes and matches `MuxMsg::RequestDevices`
4. **active_connections.get_all()** collects connected device UUIDs:
   - Locks the internal LinearMap
   - Iterates entries
   - Builds a Vec of up to 8 addresses
   - Unlocks and returns
5. **Snapshot built:** `MuxMsg::DevicesSnapshot(devices)` created
6. **Sent to HOST_RESPONSES channel** with `.send(response).await`
7. **host_responses_tx_task()** dequeues the snapshot
8. **send_mux_msg()** encodes with postcard to write buffer
9. **USB write_transfer()** sends on bulk IN endpoint
10. **Host receives** postcard-encoded `DevicesSnapshot`

---

## No odyssey_hub_server in Codebase

The donger repository does not contain an `odyssey_hub_server` project. The device snapshot protocol is between:
- **Client:** The USB host (PC running user software)
- **Server:** The dongle-fw firmware running on nRF52840

If you are looking for a separate hub server implementation, it would be in a different repository (likely the main odyssey or ats-vision-tool repository).

---

## Protocol Documentation

See:
- **Data Plane (bulk):** `dongle-fw/README.md` - MuxMsg protocol section
- **Control Plane (EP0):** `dongle-fw/.claude/PROTOCOL_EXTENSION.md` - USB control transport
- **External Types:** `protodongers` crate at https://github.com/odysseyarm/protodonge-rs.git

