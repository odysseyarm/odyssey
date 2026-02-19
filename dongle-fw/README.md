# Dongle Firmware

nRF52840 USB dongle firmware that acts as a BLE-to-USB mux for up to 7 vm-fw devices using the Mux protocol.

## Features

- **USB Communication**: VID 0x1915, PID 0x5212
- **Mux Protocol (bulk)**: Postcard-encoded MuxMsg over USB bulk endpoints
- **Control Protocol (EP0)**: USB control endpoint for pairing and bond management
- **BLE Central**: Connects to up to 7 peripheral devices simultaneously using trouble-host with nrf-sdc
- **Device Management**: Pairing mode, bond storage, and device connection management
- **L2CAP CoC**: Credit-based flow control for efficient data transfer

## Architecture

```
┌─────────┐    USB (MuxMsg)     ┌────────────┐    BLE      ┌──────────┐
│   PC    │ ←─────────────────→ │  Dongle    │ ←─────────→ │  vm-fw   │
│         │                     │  (nRF52840)│             │ device 1 │
└─────────┘                     │            │ ←─────────→ │  vm-fw   │
                                │  - USB     │             │ device 2 │
                                │  - USB Mux │             │  vm-fw   │
                                │  - Storage │ ←─────────→ │ device 3 │
                                └────────────┘             └──────────┘
```

## Building

### Prerequisites

- Rust nightly toolchain
- `just` command runner
- `nrfdfu` tool for DFU flashing
- nRF52840 dongle with Nordic bootloader

### Build and Flash

See the [Justfile](Justfile) for available commands:

```bash
# Build release firmware
just build-release

# Flash via DFU (put dongle in bootloader mode first)
just dfu-release

# Build and flash in one step
just flash-release
```

### Development Build

```bash
# Debug build with more logging
cargo build

# Check code size
cargo size --release
```

## Memory Layout

Configured for nRF52840 with nrf-sdc (SoftDevice Controller):

- **Flash**: 1MB total
  - Application: 0x00000-0xE6000 (920KB)
  - Settings: 0xE6000-0xEC000 (24KB)

- **RAM**: 256KB total
  - Application stack and heap (managed by Embassy runtime)

## Mux Protocol (bulk)

The dongle communicates with the PC over USB bulk endpoints using the MuxMsg protocol (postcard-encoded):

### Messages from Host to Dongle:
- `RequestDevices`: Get list of configured device UUIDs
- `SendTo(SendTo)`: Send packet to specific device
- `ReadVersion()`: Get firmware and protocol version
- `AddDevice(Uuid)`: Add device UUID to bond list *(TODO)*
- `RemoveDevice(Uuid)`: Remove device from bond list *(TODO)*

### Messages from Dongle to Host:
- `DevicesSnapshot(Vec<Uuid>)`: List of configured devices (up to 7)
- `DevicePacket(DevicePacket)`: Packet received from a device
- `ReadVersionResponse(Version)`: Protocol and firmware version information

## BLE Implementation

### trouble-host with nrf-sdc

Uses Embassy's trouble-host BLE stack with Nordic's SoftDevice Controller (nrf-sdc):
- **Central role** with support for up to 7 simultaneous connections
- **L2CAP CoC** (Connection-oriented Channels) for efficient data transfer
- **Pairing and bonding** with persistent bond storage
- **Connection management** with automatic reconnection

### Connection Flow

1. Device enters pairing mode when requested via USB control endpoint
2. Scans for advertising peripherals and captures new devices
3. Bonds are persisted to flash storage
4. On startup, automatically connects to all bonded devices
5. L2CAP CoC channels established for bidirectional data transfer
6. Routes packets between USB bulk endpoints and BLE L2CAP channels

## Current Status

### ✅ Implemented
- [x] USB device with MuxMsg (bulk) protocol
- [x] USB control endpoint (EP0) for device management
- [x] BLE central role with trouble-host stack
- [x] Multi-device connection management (up to 7)
- [x] L2CAP credit-based flow control for data transfer
- [x] Persistent storage for device bonds (flash via cfg-noodle)
- [x] BLE bonding and pairing with security
- [x] Reconnection logic for dropped connections
- [x] Pairing mode with timeout and LED indicators
- [x] Bond clearing and device management
- [x] Bidirectional data routing framework

### 🚧 TODO
- [ ] Add `AddDevice`/`RemoveDevice` to data/control protocols

## Development

### Logging

Defmt logging is enabled. View logs with:
- `probe-rs` if using a debug probe
- RTT viewer for real-time logging

Log level can be changed in `.cargo/config.toml`:
```toml
[env]
DEFMT_LOG = "info"  # Options: trace, debug, info, warn, error
```

### Debugging

With RTT feature enabled:
```bash
# Build with RTT logging
just build-rtt

# Monitor logs with probe-rs
probe-rs run --chip nRF52840_xxAA target/thumbv7em-none-eabihf/release/dongle-fw
```

Without RTT (USB serial logging):
- Logs are sent over USB serial interface
- Use serial monitor to view defmt logs

### Testing

1. Flash application firmware via DFU
2. Connect to PC via USB
3. Enter pairing mode via USB control endpoint
4. Start vm-fw devices in pairing mode
5. Devices will bond and connect automatically
6. Use protodonge-rs client to communicate

## Troubleshooting

### Connection Issues

- Ensure vm-fw devices are advertising and in pairing mode
- Check that devices are bonded (use ClearBonds to reset)
- Verify pairing timeout hasn't expired
- Check BLE connection count hasn't exceeded 7 devices

### USB Not Detected

- Verify VID/PID (0x1915:0x5212)
- Check USB cable supports data (not charge-only)
- Try different USB port
- Check WinUSB driver installed (Windows)

### Pairing Issues

- Use StartPairing control message with appropriate timeout
- Blue LED should flash during pairing mode
- Bonded devices are stored in flash and persist across resets
- Use ClearBonds to remove all bonds if needed

## See Also

- [protodonge-rs](https://github.com/odysseyarm/protodonge-rs) - Mux and USB Mux control protocol definitions
- [trouble-host](https://github.com/embassy-rs/trouble) - Embassy BLE host stack
- [nrf-sdc](https://github.com/alexmoon/nrf-sdc) - Nordic SoftDevice Controller bindings

## USB Control (EP0)

A separate control plane is carried over EP0 control transfers using UsbMuxCtrlMsg with interface recipient on interface 0:

- OUT (host->device) SEND: bmRequestType Vendor|Interface|OUT, bRequest 0x30, wValue 0, wIndex 0, data = postcard(UsbMuxCtrlMsg)
- IN (device->host) RECV: bmRequestType Vendor|Interface|IN, bRequest 0x31, wValue 0, wIndex 0, data = postcard(UsbMuxCtrlMsg)

Control messages include:
- ReadVersion / ReadVersionResponse(UsbMuxVersion)
- ListBonds / ListBondsResponse(Vec<Uuid>)
- StartPairing(StartPairing) / StartPairingResponse
- CancelPairing
- PairingResult(Result<Uuid, PairingError>)
- ClearBonds / ClearBondsResponse(Result<(), ClearBondsError>)
