# Device Snapshot - Architecture and Data Flow

Complete architectural overview of the device snapshot system in dongle-fw.

---

## System Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         USB Host (PC)                           │
│                   Sends: RequestDevices                         │
│                   Receives: DevicesSnapshot                     │
└──────────────────────────────────────────────────────────────────┘
                                 │
                       USB Bulk Endpoints
                    (postcard-encoded MuxMsg)
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────┐
│                    nRF52840 Dongle Firmware                     │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │              USB Interface & Communication                │  │
│  │                                                            │  │
│  │  ┌────────────────────┐          ┌────────────────────┐  │  │
│  │  │  usb_rx_task       │          │ usb_tx_task        │  │  │
│  │  │  Bulk OUT (512B)   │          │ Bulk IN (256B)     │  │  │
│  │  │                    │          │                    │  │  │
│  │  │  Read buffer       │          │ Send buffer        │  │  │
│  │  │  ↓                 │          │ ↑                  │  │  │
│  │  │ [512 bytes] ───────┼──────────┼────────[256 bytes] │  │  │
│  │  │                    │          │                    │  │  │
│  │  └────────────────────┘          └────────────────────┘  │  │
│  │           │                              ▲                │  │
│  │           ▼                              │                │  │
│  │      handle_usb_rx()          host_responses_tx_task()   │  │
│  │      ┌────────────────┐       ┌────────────────────────┐ │  │
│  │      │ Decode MuxMsg  │       │ Dequeue response       │ │  │
│  │      │ (postcard)     │       │ Encode MuxMsg          │ │  │
│  │      │                │       │ (postcard)             │ │  │
│  │      │ Match on       │       │                        │ │  │
│  │      │ RequestDevices │       │ send_mux_msg()         │ │  │
│  │      └────────────────┘       └────────────────────────┘ │  │
│  │           │                                  ▲             │  │
│  │           ▼                                  │             │  │
│  │      Call: get_all()      HOST_RESPONSES Channel          │  │
│  │           │               (capacity: 4 messages)          │  │
│  │           │                       ▲                       │  │
│  │           └───────────────────────┘                       │  │
│  │                (send())                                    │  │
│  │                                                            │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │           BLE Connection Management                       │  │
│  │                                                            │  │
│  │     ActiveConnections (AsyncMutex protected)              │  │
│  │     ┌──────────────────────────────────────┐              │  │
│  │     │  Device 1: [6 bytes]                 │              │  │
│  │     │  Device 2: [6 bytes]  ← Collected by │              │  │
│  │     │  Device 3: [6 bytes]    get_all()    │              │  │
│  │     │  Device 4: [6 bytes]                 │              │  │
│  │     │  ... (up to 8 devices)               │              │  │
│  │     └──────────────────────────────────────┘              │  │
│  │                   ▲                 ▲                      │  │
│  │                   │                 │                      │  │
│  │            add() on connect   remove() on disconnect      │  │
│  │                   │                 │                      │  │
│  │        ┌──────────┴─────────┬───────┴──────────┐          │  │
│  │        ▼                    ▼                  ▼          │  │
│  │  central_tx_task    central_conn_task   central_data_rx  │  │
│  │  (control channel)   (connection mgmt)   (data channel)   │  │
│  │        ↓                    ↓                  ↓          │  │
│  │  ┌──────────────────────────────────────────────────┐    │  │
│  │  │          BLE L2CAP Channels per Device           │    │  │
│  │  │  (2 channels: control + data, up to 8 devices)   │    │  │
│  │  └──────────────────────────────────────────────────┘    │  │
│  │                                                            │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
                                 │
                       BLE Radio (trouble-host)
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────┐
│                     BLE Peripheral Devices                      │
│            (vm-fw device 1, 2, 3, ... up to 8)                │
└─────────────────────────────────────────────────────────────────┘
```

---

## Data Flow Diagram: RequestDevices → DevicesSnapshot

```
Time →

Host                              Dongle-FW                BLE Devices
 │                                   │                         │
 │                                   │                         │
 │─ (1) RequestDevices (MuxMsg) ────>│                         │
 │      Postcard encoded             │                         │
 │      (bulk OUT endpoint)           │                         │
 │                                   │                         │
 │                                   ├─ (2) Decode              │
 │                                   │      postcard            │
 │                                   │                         │
 │                                   ├─ (3) Match on            │
 │                                   │      RequestDevices      │
 │                                   │                         │
 │                                   ├─ (4) acquire lock on     │
 │                                   │      ActiveConnections   │
 │                                   │                         │
 │                                   ├─ (5) iterate map         │
 │                                   │      of connected devs   │
 │                                   │      ↓                   │
 │                                   │  [Device 1 addr]        │
 │                                   │  [Device 2 addr]        │
 │                                   │  [Device 3 addr]        │
 │                                   │                         │
 │                                   ├─ (6) release lock        │
 │                                   │                         │
 │                                   ├─ (7) build response:     │
 │                                   │      DevicesSnapshot     │
 │                                   │                         │
 │                                   ├─ (8) send() to           │
 │                                   │      HOST_RESPONSES ch   │
 │                                   │                         │
 │                                   │      [queued in ch]      │
 │                                   │                         │
 │                                   ├─ (9) dequeue from        │
 │                                   │      HOST_RESPONSES      │
 │                                   │                         │
 │                                   ├─ (10) encode with        │
 │                                   │       postcard           │
 │                                   │                         │
 │<─ (11) DevicesSnapshot ──────────┤                         │
 │       Postcard encoded            │                         │
 │       (bulk IN endpoint)           │                         │
 │                                   │                         │
 │─ (12) Parse response ─────────────>                        │
 │                                   │                         │
 │ Done: devices = [addr1, addr2...]  │                         │
 │
```

---

## Concurrency Model

### Task Spawning (from `main()`)

```
main task (initialization)
  │
  ├─ spawner.must_spawn(usb_rx_task)
  │   └─ Receives RequestDevices on bulk OUT
  │      └─ Calls handle_usb_rx()
  │         └─ Locks ActiveConnections
  │            └─ Sends to HOST_RESPONSES channel
  │
  ├─ spawner.must_spawn(usb_tx_task)
  │   └─ Spawns: host_responses_tx_task
  │   │   └─ Dequeues DevicesSnapshot from HOST_RESPONSES
  │   │      └─ Sends on bulk IN endpoint
  │   │
  │   └─ Spawns: device_packets_tx_task
  │       └─ Sends device packets (independent)
  │
  ├─ spawner.must_spawn(ble_task)
  │   └─ BleManager::run()
  │      └─ connect_to_device()
  │         └─ central_tx_task (modifies ActiveConnections)
  │         └─ central_control_rx_task
  │         └─ central_data_rx_task
  │         └─ central_conn_task (modifies ActiveConnections)
  │
  └─ ... other tasks (pairing, bond storage, etc.)
```

### Synchronization Points

1. **ActiveConnections Lock** (AsyncMutex)
   - Locked by `get_all()` (read-only iteration)
   - Locked by `add()` (insert)
   - Locked by `remove()` (delete)
   - **No starvation risk:** AsyncMutex is fair

2. **HOST_RESPONSES Channel**
   - Multiple tasks can send (bounded to 4 messages)
   - One task dequeues (host_responses_tx_task)
   - **Blocking if full:** sender blocks until slot available

3. **USB Endpoint Mutex** (in usb_tx_task)
   - Protects concurrent access to bulk IN endpoint
   - Ensures serialized writes to USB hardware

---

## Connection Lifecycle vs Snapshot

```
Timeline:
         Device connects         Host requests snapshot
              │                           │
              ▼                           ▼
         ┌─────────────┐         ┌──────────────────┐
         │ central_tx  │         │ handle_usb_rx    │
         │ central_rx  │         │ (RequestDevices) │
         │ central_con │         │                  │
         └─────────────┘         └──────────────────┘
              │                           │
              │ add() to                  │
              │ ActiveConnections         │ get_all() from
              │ ▼                         │ ActiveConnections
              ├─► Device appears         │ ▼
              │   in map                 ├─► Device included
              │                          │   in snapshot
              │                          │
             [Device connected]         [Snapshot sent to host]
              │
              │ (time passes)
              │
              ▼
         [Device disconnects]
              │
              │ remove() from
              │ ActiveConnections
              ▼
         [Device no longer in map]
         [Won't appear in next snapshot]
```

---

## Memory Layout

### ActiveConnections Structure

```
ActiveConnections {
    connections: AsyncMutex<
        ThreadModeRawMutex,
        LinearMap<[u8; 6], (), 8>  // MAX_DEVICES = 8
    >
}

Size per entry: 6 bytes (address) + 0 bytes (unit value) = 6 bytes
Maximum total: 8 × 6 = 48 bytes (resident)

Snapshot Vec during get_all():
    HeaplessVec<[u8; 6], 8>
    Size: ~50 bytes on stack
```

### USB Communication Buffers

```
usb_rx_task:
    ├─ read_buf: [u8; 512]        // Bulk OUT buffer
    └─ Decoded MuxMsg (stack var)

usb_tx_task (host_responses_tx_task):
    ├─ write_buf: [u8; 256]       // Bulk IN buffer
    ├─ Encoded DevicesSnapshot (in write_buf)
    │  └─ ~2 bytes length + (num_devices × 6 bytes)
    │     Example: 2 + (3 × 6) = 20 bytes payload
    │
    └─ Actual transfer: minimal overhead with postcard

HOST_RESPONSES channel:
    ├─ Capacity: 4 MuxMsg enums
    ├─ Each MuxMsg enum ~64 bytes (varies by variant)
    └─ Max buffer: 4 × 64 = 256 bytes
```

---

## Control Plane vs Data Plane

### Data Plane (Bulk Endpoints) - Device Snapshot

```
Endpoints: Bulk OUT (host → device), Bulk IN (device → host)
Message Type: MuxMsg enum
Encoding: postcard
Latency: ~1-10ms
Examples:
  - RequestDevices / DevicesSnapshot
  - SendTo / DevicePacket
  - ReadVersion / ReadVersionResponse
```

### Control Plane (EP0) - Separate

```
Endpoint: Control (EP0)
Message Type: UsbMuxCtrlMsg enum
Encoding: postcard
Latency: Variable (host-polled)
Examples:
  - StartPairing / StartPairingResponse
  - CancelPairing
  - ClearBonds / ClearBondsResponse
  - ReadVersion / ReadVersionResponse (duplicate on control plane)

Implementation: dongle-fw/src/control.rs
Status: Defined, used for pairing/bonding, snapshot NOT on this plane
```

The separation allows device packets and pairing events to not interfere with each other.

---

## Error Handling

### RequestDevices Path

```
handle_usb_rx()
  │
  ├─ postcard::from_bytes()
  │  ├─ Error: log and return Err
  │  └─ No response sent to host
  │
  ├─ Match MuxMsg::RequestDevices
  │  └─ Guaranteed to succeed
  │
  ├─ active_connections.get_all()
  │  └─ Cannot fail (returns empty Vec if no connections)
  │
  ├─ Build response
  │  └─ Cannot fail
  │
  └─ HOST_RESPONSES.send()
     ├─ Ok: Queued
     └─ Err: Channel full (unlikely, capacity 4)
        └─ Message silently dropped
```

### Response Transmission Path

```
host_responses_tx_task()
  │
  ├─ HOST_RESPONSES.receive()
  │  └─ Blocks until message available
  │
  ├─ postcard::to_slice()
  │  └─ Error: break from task (USB TX error condition)
  │
  └─ EndpointIn::write_transfer()
     ├─ Ok: Data sent to host
     └─ Err: USB disconnected, break from task
```

---

## Key Constants and Limits

| Constant | Value | Impact |
|----------|-------|--------|
| `MAX_DEVICES` | 8 | Max connected devices, snapshot size ~50 bytes |
| `HOST_RESPONSES` capacity | 4 | Can queue up to 4 responses (RequestDevices, ReadVersion, etc.) |
| Bulk OUT buffer | 512 bytes | Max RequestDevices packet size |
| Bulk IN buffer | 256 bytes | Max DevicesSnapshot packet size (8×6=48 + overhead) |
| Snapshot Vec capacity | 8 | Matches MAX_DEVICES |
| L2CAP channels per device | 2 | Control + data |
| Total L2CAP channels | 16 | 2 × 8 devices |

---

## Summary: Files and Responsibilities

| Module | File | Responsibility |
|--------|------|-----------------|
| **USB Interface** | `src/main.rs` | Endpoint setup, RX/TX tasks, message routing |
| **USB RX** | `src/main.rs` | `usb_rx_task`, `handle_usb_rx` |
| **USB TX** | `src/main.rs` | `usb_tx_task`, `host_responses_tx_task`, `send_mux_msg` |
| **Device Tracking** | `src/ble/central.rs` | `ActiveConnections` struct and methods |
| **Device Queue Management** | `src/ble/central.rs` | `DeviceQueues`, per-device packet routing |
| **BLE Connection Management** | `src/ble/central.rs` | `BleManager`, `connect_to_device`, connection tasks |
| **Control Plane** | `src/control.rs` | EP0 pairing/bonding (separate from snapshots) |
| **USB Configuration** | `src/usb.rs` | Device descriptor, vendor IDs (0x1915:0x5210) |

