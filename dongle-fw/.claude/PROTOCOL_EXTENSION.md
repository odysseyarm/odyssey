# Protocol Extensions: USB Mux Control + Device Management

This document sketches the control-plane protocol (over USB EP0) and the
next steps to extend device management. It complements the bulk data-plane
(MuxMsg over USB bulk endpoints).

## Overview

- Data plane (bulk): postcard-encoded `MuxMsg` over USB bulk IN/OUT.
- Control plane (EP0): postcard-encoded `UsbMuxCtrlMsg` exchanged via
  USB control transfers addressed to interface 1 (recipient=Interface).

These planes are independent so control traffic does not contend with bulk
streaming.

## USB Control Transport (EP0)

Use WinUSB-compatible vendor interface control requests:

- OUT SEND (host → device)
  - bmRequestType: Vendor | Interface | OUT (0x41)
  - bRequest: 0x30
  - wValue: 0
  - wIndex: 1 (interface number)
  - Data: postcard(`UsbMuxCtrlMsg`)

- IN RECV (device → host)
  - bmRequestType: Vendor | Interface | IN (0xC1)
  - bRequest: 0x31
  - wValue: 0
  - wIndex: 1
  - Data: postcard(`UsbMuxCtrlMsg`)

The device buffers one or more control events; the host calls RECV to pop one.

## Control Messages (current)

- Version
  - `ReadVersion()`
  - `ReadVersionResponse(UsbMuxVersion)`

- Pairing
  - `StartPairing(StartPairing { timeout_ms })`
  - `StartPairingResponse`
  - `CancelPairing`
  - `PairingResult(Result<Uuid, PairingError>)`

- Bond store
  - `ClearBonds`
  - `ClearBondsResponse(Result<(), ClearBondsError>)`
  - `BondStoreError(BondStoreError)` (async event on capacity issues)

Types are defined in `protodonge-rs/src/control/usb_mux.rs`.

## Proposed Device-Management Extensions (control plane)

- List bonds (host fetches known devices)
  - `ListBonds()`
  - `ListBondsResponse(HVec<Uuid, MAX_DEVICES>)`

- Add device UUID (pre-provision target without pairing flow)
  - `AddDevice(Uuid)`
  - `AddDeviceResponse(Result<(), BondStoreError>)`

- Remove device UUID
  - `RemoveDevice(Uuid)`
  - `RemoveDeviceResponse(Result<(), ()>)`

Notes:
- `Uuid` here is the 6-byte address (BLE BdAddr) wrapped by the Mux protocol
  type; we reuse it for consistency with data-plane addressing.
- For constrained buffers, these can be delivered as separate request/response
  pairs, not as streaming snapshots.

## Data Plane (bulk) – for reference

- Host → device:
  - `RequestDevices`
  - `SendTo(SendTo { dev: Uuid, pkt: Packet })`
  - `ReadVersion()`

- Device → host:
  - `DevicesSnapshot(HVec<Uuid, MAX_DEVICES>)`
  - `DevicePacket(DevicePacket)`
  - `ReadVersionResponse(Version)`

All Mux messages are postcard-encoded over bulk endpoints.

## Compatibility & Versioning

- The control-plane version is reported in `UsbMuxVersion::protocol_semver`.
- While iterating on v0.1.0, host does not strictly gate on version.
- When extensions above are implemented, bump minor version.

## Open Items

- Exact queue depth for control events and behavior when empty/full.
- Whether `ListBonds` should page when bonds exceed `MAX_DEVICES`.
- Host retry/backoff recommendations for `RECV` when no events are pending.
