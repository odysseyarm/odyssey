# Report transport

Odyssey transports sensor reports as typed `PacketData` values over USB and BLE. Firmware reduces the representation of reports without changing their measurement values. Transfer packing reduces the number of transport submissions without changing report order.

## Protocol boundaries

| Version | Contract | Reported by |
|---|---|---|
| Device protocol | The `Packet` and `PacketData` wire representation, including sensor reports. | The unit's events-protocol version response. |
| Mux protocol | The dongle's `MuxMsg` envelope, addressing and dongle operations. | The dongle's mux version response. |
| Firmware version | The implementation release installed on a device. | The firmware component of its version response. |

A mux device packet contains a device-protocol packet. Appending a device report variant changes the device protocol even when the mux envelope remains unchanged. A dongle must understand the nested device protocol to deserialize and relay that packet. Its minimum Desktop requirement therefore covers both protocol surfaces.

Version values from independent control endpoints must not be compared with the events protocol unless their endpoint explicitly reports that same contract. A firmware release version is not a substitute for a protocol capability.

## Report representation

`SparseMarkersReport` carries an occupancy mask and the occupied marker positions in slot order. Expansion restores the positions to their original slots and fills the empty slots with the origin. The coordinates retain their original integer precision. A dense report can use `MarkersReport` when its encoding is shorter.

`AccelBatch` carries a bounded sequence of finished IMU reports. Each report keeps its timestamp and its acceleration and rotation-rate float bits. The outer packet id identifies the IMU subscription. Reports with different ids cannot share an IMU batch.

The variants are ordinary protocol values with schema entries. The dongle forwards them inside `MuxMsg::DevicePacket` without expanding them. The host expands compact reports before dispatch to the existing marker and IMU subscriptions. The tracking code and SDK callbacks continue to receive individual reports.

## Averaging and measurement time

The unit's IMU integrator processes raw samples before transport batching. Each emitted report describes motion over the interval ending at its timestamp. Batching neither averages these reports again nor replaces their timestamps with arrival times.

The hub advances its filter from consecutive report timestamps. A burst of arrivals therefore preserves each report's measurement interval. Saturated samples remain detectable because transport preserves the float bits. Transport batching does not repair a report dropped before transmission.

The reporting rate is configurable. The default rate is not a transport capacity guarantee. Increasing it spends radio airtime, queue capacity and host processing time. A sustainable rate must be measured with all supported tracker connections, marker traffic and retransmissions present.

## Transfer packing and ordering

One BLE data SDU or USB transfer can carry several complete postcard packets consecutively. The dongle's USB transfer can likewise carry several complete mux messages. Readers consume every message with postcard's remainder-aware decoder; decoding only the first value would silently discard the rest.

Typed batching and transfer packing are separate operations. An IMU batch and a sparse marker packet can share a transfer. Each packet keeps its own subscription id. A sender must not move an IMU report across a marker, impact or other queued report to improve packing. Control responses keep their control-channel routing. Large image packets remain valid even when they exceed the preferred packing target.

Packing is bounded by encoded bytes, not by a required number of reports. The IMU batch capacity is eight reports. This is a maximum, not a target that the sender waits to fill.

## Radio and USB budgets

The BLE data channel configures an MPS of 247 bytes in `firmware/atslite-common/src/ble/peripheral.rs`. The first credit-based L2CAP PDU also carries a two-byte SDU length. A 245-byte SDU packing target therefore fits that configured first-PDU budget. Actual link-layer fragmentation still depends on negotiated data length and controller behavior.

The unit uses the same 245-byte packing target for direct USB reports. A standalone packet larger than the target is sent on its own. The dongle's USB sender packs mux messages into a buffer sized for a 1024-byte BLE packet plus the one-byte mux tag and six-byte address, as configured in `firmware/dongle-fw/src/main.rs`.

The dongle requests a 7.5 ms connection interval in `firmware/dongle-fw/src/ble/device_task.rs`. An interval separates connection events; it is not a fixed-size payload frame. An event can carry several radio packets. A transfer that needs another fragment does not necessarily need another event.

USB full-speed bulk endpoints use 64-byte packets. A larger transfer spans several packets. It does not reserve an entire 1 ms USB frame. Full packets can reduce transaction overhead, but host scheduling still determines when bulk traffic runs.

## Latency policy

The sender waits for the first report, then considers only reports already queued. It has no timer and does not wait for a full batch. This policy adds no intentional accumulation delay. Serialization, controller queueing and transmission still take time.

A connection interval does not make an arbitrary accumulation timer free. A delayed submission can miss the controller's opportunity for the next connection event. Event-aligned flushing would require a scheduling contract that this implementation does not have.

## Compatibility and updates

Firmware implementing device protocol 0.4.0 sends the compact variants without capability negotiation. Readers also accept the established individual report variants. The release manifest's minimum Desktop requirement prevents an incompatible Desktop from offering the firmware update.

The manifest floor does not prevent a device updated elsewhere from being connected to an incompatible Desktop. Such a Desktop cannot decode the compact reports. This is an accepted compatibility limit, not a runtime handshake.

The dongle must be updated before the units. A dongle without the compact variants drops those reports while its control and update paths remain available. The Desktop warning on the dongle update card appears only when an update is offered.

## Verification limits

Host tests can check exact report reconstruction, bounded decoding, mixed-message transfers and independence from earlier batches. They cannot establish radio latency, controller fragmentation, sustainable reporting rates or full-speed USB throughput. Those require hardware measurements. No live firmware release is approved by these host checks alone.
