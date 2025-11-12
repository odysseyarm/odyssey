# Device Snapshot - Code Snippets Reference

Quick reference for the key code locations involved in device snapshot generation.

---

## 1. RequestDevices Message Handler

**File:** `D:\ody\donger\dongle-fw\src\main.rs` (lines 619-665)

```rust
async fn handle_usb_rx(
    data: &[u8],
    device_queues: &ble::central::DeviceQueues,
    active_connections: &ActiveConnections,
) -> Result<(), ()> {
    // Decode using postcard (standard for USB in ats_usb)
    let msg: MuxMsg = postcard::from_bytes(data).map_err(|e| {
        error!(
            "Failed to deserialize USB packet ({} bytes): {:?}",
            data.len(),
            defmt::Debug2Format(&e)
        );
        ()
    })?;

    match msg {
        MuxMsg::RequestDevices => {
            let devices = active_connections.get_all().await;
            let response = MuxMsg::DevicesSnapshot(devices);
            HOST_RESPONSES.send(response).await;
        }
        MuxMsg::SendTo(send_to_msg) => {
            // Route message to the specific device's queue
            let device_uuid = &send_to_msg.dev;
            info!("SendTo message for device {:02x}", device_uuid);
            if let Some(queue) = device_queues.get_queue(device_uuid).await {
                let device_pkt = DevicePacket {
                    dev: send_to_msg.dev,
                    pkt: send_to_msg.pkt,
                };
                match queue.try_send(device_pkt) {
                    Ok(()) => {
                        info!("Packet queued successfully for device {:02x}", device_uuid);
                    }
                    Err(_) => {
                        warn!("Device queue full for {:02x}, packet dropped", device_uuid);
                    }
                }
            } else {
                error!(
                    "No queue found for device {:02x} - device not connected",
                    device_uuid
                );
            }
        }
        MuxMsg::ReadVersion() => {
            let response = MuxMsg::ReadVersionResponse(VERSION);
            HOST_RESPONSES.send(response).await;
        }
        _ => {
            error!(
                "Unexpected message from host: {:?}",
                defmt::Debug2Format(&msg)
            );
        }
    }

    Ok(())
}
```

---

## 2. Snapshot Generation - get_all()

**File:** `D:\ody\donger\dongle-fw\src\ble\central.rs` (lines 160-220)

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

    pub async fn add(&self, uuid: &Uuid) -> bool {
        let mut map = self.connections.lock().await;
        map.insert(*uuid, ()).is_ok()
    }

    pub async fn remove(&self, uuid: &Uuid) {
        let mut map = self.connections.lock().await;
        let _ = map.remove(uuid);
    }

    pub async fn contains(&self, uuid: &Uuid) -> bool {
        let map = self.connections.lock().await;
        map.contains_key(uuid)
    }

    #[allow(dead_code)]
    pub async fn count(&self) -> usize {
        let map = self.connections.lock().await;
        map.len()
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

**Critical:** The `get_all()` method:
1. Acquires async lock on the connections map
2. Iterates all entries
3. Collects UUIDs into a heap-allocated vector
4. Returns the vector (automatically serializable by postcard)

---

## 3. Response Transmission via USB

**File:** `D:\ody\donger\dongle-fw\src\main.rs` (lines 21-23, 562-578)

```rust
// Channel for sending host responses (RequestDevices, ReadVersion, etc.)
type HostResponseChannel = Channel<ThreadModeRawMutex, MuxMsg, 4>;
static HOST_RESPONSES: HostResponseChannel = Channel::new();

// ... later ...

// Sends host responses (RequestDevices, ReadVersion, etc.) to USB endpoint
async fn host_responses_tx_task(
    ep_mutex: &'static embassy_sync::mutex::Mutex<
        ThreadModeRawMutex,
        Option<embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>>,
    >,
) {
    let mut write_buf = [0u8; 256];

    loop {
        let response = HOST_RESPONSES.receive().await;

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

---

## 4. USB Message Encoding and Transmission

**File:** `D:\ody\donger\dongle-fw\src\main.rs` (lines 600-612)

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

---

## 5. USB RX Task - Entry Point

**File:** `D:\ody\donger\dongle-fw\src\main.rs` (lines 490-520)

```rust
#[embassy_executor::task]
async fn usb_rx_task(
    mut ep_out: embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::Out>,
    device_queues: &'static ble::central::DeviceQueues,
    active_connections: &'static ActiveConnections,
) {
    loop {
        info!("USB RX task: waiting for USB configuration");

        let mut read_buf = [0u8; 512];

        loop {
            match EndpointOut::read_transfer(&mut ep_out, &mut read_buf).await {
                Ok(n) => {
                    info!("USB RX: received {} bytes", n);
                    if let Err(_e) =
                        handle_usb_rx(&read_buf[..n], device_queues, active_connections).await
                    {
                        error!("Error handling USB RX ({} bytes)", n);
                    }
                }
                Err(EndpointError::Disabled) => {
                    info!("USB RX: disabled, waiting for reconnection");
                    break;
                }
                Err(_e) => {
                    warn!("USB RX: read error");
                }
            }

            if pairing::take_timeout() {
                // Emit control-plane timeout event
                control::try_send_event(UsbMuxCtrlMsg::PairingResult(Err(PairingError::Timeout)));
            }
        }
    }
}
```

---

## 6. USB TX Task - Response Transmission

**File:** `D:\ody\donger\dongle-fw\src\main.rs` (lines 531-560)

```rust
#[embassy_executor::task]
async fn usb_tx_task(
    ep_in: embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>,
    device_packets: &'static DevicePacketChannel,
) {
    loop {
        info!("USB TX task: waiting for USB configuration");

        // Wrap endpoint in a mutex so multiple tasks can share it
        static EP_MUTEX: StaticCell<
            embassy_sync::mutex::Mutex<
                ThreadModeRawMutex,
                Option<embassy_nrf::usb::Endpoint<'static, embassy_nrf::usb::In>>,
            >,
        > = StaticCell::new();
        let ep_mutex = EP_MUTEX.init(embassy_sync::mutex::Mutex::new(Some(ep_in)));

        // Spawn two independent tasks: one for host responses, one for device packets
        let host_task = host_responses_tx_task(ep_mutex);
        let device_task = device_packets_tx_task(ep_mutex, device_packets);

        // Run both tasks concurrently - neither will be cancelled
        use embassy_futures::join::join;
        let _ = join(host_task, device_task).await;

        warn!("USB TX: one of the tasks ended, waiting for reconfiguration");
        break;
    }
}
```

---

## 7. Connection Add - When Device Connects

**File:** `D:\ody\donger\dongle-fw\src\ble\central.rs` (lines 840-860)

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

// Split L2CAP channels into separate readers and writers
let (control_writer, control_reader) = l2cap_control_channel.split();
let (data_writer, data_reader) = l2cap_data_channel.split();
```

---

## 8. Connection Remove - When Device Disconnects

**File:** `D:\ody\donger\dongle-fw\src\ble\central.rs` (lines 900-920)

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
                active_connections.remove(&_target.into_inner()).await;
                device_queues.unregister(&_target.into_inner()).await;
                return;
            }
            trouble_host::prelude::ConnectionEvent::PhyUpdated { tx_phy, rx_phy } => {
                info!("PHY updated tx={:?} rx={:?}", tx_phy, rx_phy);
            }
            trouble_host::prelude::ConnectionEvent::PairingComplete {
                security_level,
                bond,
            } => {
                info!("Pairing complete: {:?}", security_level);
                if let Some(bi) = bond {
                    let bd = crate::ble::security::bonddata_from_info(&bi);
                    info!("Storing bond for {:02x}", bd.bd_addr);
                    crate::ble::security::BOND_TO_STORE.signal(bd);
                }
            }
            trouble_host::prelude::ConnectionEvent::PairingFailed(err) => {
                error!("Pairing failed: {:?}", err);
            }
            _ => {}
        }
    }
}
```

---

## 9. Global Initialization

**File:** `D:\ody\donger\dongle-fw\src\main.rs` (lines 175-385)

Key initialization in `main()`:

```rust
// Initialize active connections tracker
let active_connections = ACTIVE_CONNECTIONS.init(ActiveConnections::new());

// ... later ...

// Store global references for use in control task
ble::central::set_global_refs(active_connections, device_queues).await;

spawner.must_spawn(usb_rx_task(ep_out, device_queues, active_connections));
spawner.must_spawn(usb_tx_task(ep_in, device_packets));
```

---

## 10. Type Definitions

**File:** `D:\ody\donger\dongle-fw\src\ble\central.rs` (lines 67-70)

```rust
pub type Uuid = [u8; 6];  // 6-byte BLE address, not a standard UUID
```

**From protodongers crate:**

```rust
pub enum MuxMsg {
    RequestDevices,
    SendTo(SendTo { dev: Uuid, pkt: Packet }),
    ReadVersion(),
    DevicesSnapshot(HVec<Uuid, MAX_DEVICES>),  // Response type for snapshot
    DevicePacket(DevicePacket),
    ReadVersionResponse(Version),
}
```

---

## Summary Table: Code Flow

| Step | Function | File | Action |
|------|----------|------|--------|
| 1 | `usb_rx_task` | main.rs | Receives bulk OUT data from host |
| 2 | `handle_usb_rx` | main.rs | Decodes postcard, matches RequestDevices |
| 3 | `ActiveConnections::get_all` | central.rs | Locks map, collects UUIDs, returns Vec |
| 4 | `HOST_RESPONSES.send` | main.rs | Queues DevicesSnapshot to response channel |
| 5 | `host_responses_tx_task` | main.rs | Dequeues response from channel |
| 6 | `send_mux_msg` | main.rs | Encodes with postcard |
| 7 | `EndpointIn::write_transfer` | main.rs | Sends encoded data on bulk IN endpoint |

