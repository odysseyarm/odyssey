# Report streaming

A unit streams sensor reports to the host as typed `PacketData` values. The reports travel over USB while a USB host has the unit configured and over BLE otherwise. Firmware shortens the encoding of reports without changing their measurement values. Senders pack queued packets into shared transfers without changing their order.

The wire contracts live with the protocol definitions: [packet variants](../../crates/odyssey-protocol/src/packet.rs), [sensor report payloads](../../crates/odyssey-protocol/src/reports.rs), [device versions](../../crates/odyssey-protocol/src/version.rs) and [mux messages](../../crates/odyssey-protocol/src/mux.rs). This page explains the encoding, packing and release decisions that use those contracts.

## Terms

| Term | Meaning |
|---|---|
| Stream | The reports of one kind that the host enables with `StreamUpdate`. Each report carries the id of the request that enabled its stream. |
| Report | One measurement: marker positions, IMU motion, an impact or the battery state. |
| Packet | A `Packet`: one `PacketData` value and an id. |
| Transfer | One USB bulk transfer or one BLE L2CAP SDU. A transfer carries one or more packets. |
| Transport | The link that carries a unit's reports: direct USB or BLE (`TransportMode`). |

## Path of a report

| Stage | Operation | Code |
|---|---|---|
| Unit sensor loops | Queue one packet per report. The queue holds 64 packets, and a report that finds it full is dropped. | `firmware/atslite-common/src/object_mode.rs` |
| Unit sender | Packs the queued packets into one transfer and joins adjacent IMU reports. | `pack_queued` in `crates/odyssey-protocol/src/transfer.rs`, called through `firmware/atslite-common/src/packing.rs` |
| Dongle | Relays each device packet inside `MuxMsg::DevicePacket` without expanding it. Packs the queued mux messages into one USB transfer. | `firmware/dongle-fw/src/ble/device_task.rs`, `firmware/dongle-fw/src/main.rs` |
| Host readers | Decode every packet of a transfer. | `crates/odyssey-usb/src/device/usb_bulk_io.rs`, `crates/odyssey-usb/src/device/l2cap_transport.rs` |
| Host dispatcher | Splits the compact report forms into single dense reports and delivers each to the stream of its id. | `deliver_expanded` in `crates/odyssey-usb/src/device/vm_device.rs` |

The tracking code and the SDK callbacks receive individual reports on every path.

The unit packs at the point of transmission on both transports. On direct USB the stream loop packs from the report queue. On BLE the stream loop moves each report into an 8-packet data queue, and the data-channel writer packs from that queue. The legacy ATS Lite firmware shares this BLE writer.

## Compact report forms

`SparseMarkersReport` carries an occupancy mask and the occupied marker positions in slot order. Expansion returns the positions to their slots and fills the empty slots with the origin. The coordinates keep their integer precision. The host rejects a report whose point count differs from its mask and logs the rejection.

The modern ATS Lite firmware sends each marker report in whichever form encodes shorter. A dense `MarkersReport` spends two bytes on each empty slot. The sparse form spends one to three bytes on the mask and one byte on the point count instead. The sparse form is never longer when two or more slots are empty, and the dense form is two to four bytes shorter otherwise. `From<MarkersReport> for PacketData` therefore sends the dense form when at most one slot is empty. These sizes follow from the postcard varint encoding, and a protocol test checks every occupancy count.

`AccelBatch` carries up to eight IMU reports (`AccelReport`) of one stream in measurement order. Each report keeps its timestamp and the bits of its acceleration and rotation-rate floats. The outer packet id identifies the stream, so reports with different ids cannot share a batch. The capacity is a maximum. A sender never waits to fill a batch.

One IMU report in its own packet takes at most 31 bytes, so eight separate packets take up to 248 bytes. A full batch packet takes at most 235 bytes and fits the [packing target](#size-budgets). A test in `crates/odyssey-protocol/src/reports.rs` pins the batch size.

A sender joins only reports that are adjacent in its queue, through `Packet::join_accel`. On BLE this applies to the legacy ATS Lite firmware too. Its marker reports keep the `LegacyCombinedMarkersReport` form.

The compact forms are ordinary protocol values with schema entries. The dongle forwards them unchanged, and the host expands them before dispatch.

## Transfer packing and order

One BLE data SDU or one USB transfer can carry several complete postcard packets back to back. The dongle's USB transfer can likewise carry several complete mux messages. Readers consume every message with postcard's remainder-aware decoder. Decoding only the first value would silently discard the rest. A message that does not decode discards the remainder of its transfer, and the reader logs it.

Joining IMU reports and packing packets are separate operations. An IMU batch and a sparse marker packet can share a transfer, and each packet keeps its own stream id. A sender never moves a packet past another queued packet, so an IMU report does not cross a marker or impact report to improve packing.

On BLE, control responses travel on the control channel with one packet in each SDU. Direct USB has one endpoint, so queued reports can follow a reply in its transfer. A packet larger than the packing target stays valid and travels alone. Image packets are the usual case.

Packing is bounded by encoded bytes. A packet that does not fit starts the next transfer.

## Size budgets

| Quantity | Value | Source |
|---|---|---|
| BLE data-channel MPS | 247 bytes | Configured as `DATA_MPS` in `firmware/atslite-common/src/ble/peripheral.rs`: a 251-byte link-layer payload less the 4-byte L2CAP header. |
| Unit packing target | 245 bytes | `PACKING_TARGET` in `firmware/atslite-common/src/packing.rs`: the MPS less the 2-byte SDU length in an SDU's first PDU. |
| Largest IMU batch packet | 235 bytes | Computed from the postcard encoding and pinned by a protocol test. |
| Unit transfer buffer | 1024 bytes | `stream_loop` and `data_tx_task`. |
| Dongle USB transfer buffer | 1031 bytes | `firmware/dongle-fw/src/main.rs`: a 1024-byte BLE receive buffer, the 1-byte mux tag and the 6-byte device address. |
| Host transfer limit | 4096 bytes | `MAX_MESSAGE_SIZE` in `usb_bulk_io.rs` and `RECV_BUFFER_SIZE` in `l2cap_transport.rs`. |
| BLE connection interval | 7.5 ms | Requested by the dongle in `firmware/dongle-fw/src/ble/device_task.rs`. |
| USB full-speed bulk packet | 64 bytes | USB 2.0 specification. |

None of these budgets has been measured on hardware.

A 245-byte SDU fits the configured first-PDU budget. Actual link-layer fragmentation still depends on the negotiated data length and on controller behavior. The unit applies the same target to direct USB transfers. The value is a BLE budget and has not been tuned for USB.

An interval separates connection events. It is not a fixed-size payload frame. An event can carry several radio packets, and a transfer that needs another fragment does not necessarily need another event.

A USB transfer larger than 64 bytes spans several packets. It does not reserve an entire 1 ms USB frame. Full packets can reduce transaction overhead, but host scheduling still determines when bulk traffic runs.

## Latency

A sender waits for the first packet, then considers only the packets that are already queued. It has no timer and does not wait for a full batch. This policy adds no intentional accumulation delay. Serialization, controller queueing and transmission still take time.

On BLE, packets accumulate in the data queue while the writer waits for the previous SDU to leave. That wait produces the batches. A unit whose link keeps up with its reporting rate sends single reports.

A connection interval does not make an arbitrary accumulation timer free. A delayed submission can miss the controller's opportunity for the next connection event. Event-aligned flushing would require a scheduling contract that this implementation does not have.

## Measurement time

The unit's IMU integrator processes raw samples before the sender joins reports. Each emitted report describes motion over the interval ending at its timestamp. Joining neither averages these reports again nor replaces their timestamps with arrival times.

The hub advances its filter from consecutive report timestamps. A burst of arrivals therefore preserves each report's measurement interval. Saturated samples remain detectable because the encoding preserves the float bits. Joining does not repair a report dropped before transmission.

The reporting rate is configurable. The default rate is not a capacity guarantee. Increasing it spends radio airtime, queue capacity and host processing time. A sustainable rate must be measured with all supported tracker connections, marker traffic and retransmissions present.

## Protocol versions

| Version | Contract | Reported by |
|---|---|---|
| Device protocol | The `Packet` and `PacketData` wire representation, including sensor reports. | The unit's events-protocol version response. |
| Mux protocol | The dongle's `MuxMsg` envelope, addressing and dongle operations. | The dongle's mux version response. |
| Firmware version | The implementation release installed on a device. | The firmware component of its version response. |

A mux device packet contains a device-protocol packet. Appending a device report variant changes the device protocol even when the mux envelope remains unchanged. A dongle must understand the nested device protocol to deserialize and relay that packet. Its minimum Desktop requirement therefore covers both protocol surfaces.

Version values from independent control endpoints must not be compared with the events protocol unless their endpoint explicitly reports that same contract. A firmware release version is not a substitute for a protocol capability.

## Compatibility and updates

Firmware implementing device protocol 0.4.0 sends the compact forms and packed transfers without capability negotiation. Readers also accept the established individual report variants. The release manifest's minimum Desktop requirement prevents an incompatible Desktop from offering the firmware update.

The manifest floor does not prevent a device updated elsewhere from being connected to an incompatible Desktop. Such a Desktop cannot decode the compact reports. This is an accepted compatibility limit, not a runtime handshake.

The dongle must be updated before the units. A dongle without the compact variants drops those reports while its control and update paths remain available. The Desktop warning on the dongle update card appears only when an update is offered.

## Verification limits

Host tests check exact report reconstruction, bounded decoding, the order and limits of joined IMU reports, and the packing of queued packets into a transfer that decodes to the same sequence. No host test runs the firmware loops that call the packer or the dongle's packing loop, because the firmware crates cannot build for the host. The firmware builds only show that this code compiles.

Host tests cannot establish radio latency, controller fragmentation, sustainable reporting rates or full-speed USB throughput. Those require hardware measurements. No live firmware release is approved by these host checks alone.
