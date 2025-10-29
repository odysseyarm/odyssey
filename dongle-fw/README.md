# Dongle Firmware

nRF52840 USB dongle firmware that acts as a BLE-to-USB hub for up to 7 vm-fw devices using the HubMsg protocol.

## Features

- **USB Communication**: VID 0x1915, PID 0x5210
- **HubMsg Protocol**: Minicbor-encoded protocol from protodonge-rs
- **BLE Central**: Connects to up to 7 peripheral devices simultaneously using Nordic S140 SoftDevice
- **Device Management**: Add/remove devices to bond with by UUID
- **Nordic SoftDevice S140**: Officially qualified BLE stack for regulatory compliance

## Architecture

```
┌─────────┐    USB (HubMsg)     ┌────────────┐    BLE      ┌──────────┐
│   PC    │ ←─────────────────→ │  Dongle    │ ←─────────→ │  vm-fw   │
│         │                     │  (nRF52840)│             │ device 1 │
└─────────┘                     │            │ ←─────────→ │  vm-fw   │
                                │  - USB     │             │ device 2 │
                                │  - BLE Hub │             │   ...    │
                                │  - Storage │ ←─────────→ │  vm-fw   │
                                └────────────┘             │ device 7 │
                                                           └──────────┘
```

## Building

### Prerequisites

- Rust nightly toolchain
- `probe-rs` tool for flashing S140 SoftDevice
- `nrfdfu` tool for flashing application firmware
- nRF52840 dongle with Nordic bootloader
- S140 SoftDevice v7.x hex file

### First-Time Setup

1. **Flash S140 SoftDevice** (one-time, or when updating SoftDevice):
   ```bash
   probe-rs download --verify --binary-format hex --chip nRF52840_xxAA s140_nrf52_7.3.0_softdevice.hex
   ```

   Download S140 from: https://www.nordicsemi.com/Products/Development-software/s140/download

2. **Build and flash application**:
   ```bash
   # Build release firmware
   cargo build --release

   # Put dongle in bootloader mode (press reset button)
   
   # Flash to dongle
   cargo run --release
   ```

### Development Build

```bash
# Debug build with more logging
cargo build

# Check code size
cargo size --release
```

## Memory Layout

Configured for S140 SoftDevice v7.x on nRF52840:

- **Flash**: 1MB total
  - SoftDevice: 0x00000-0x26000 (152KB) - S140 v7.x
  - Application: 0x26000-0xE6000 (768KB)
  - Settings: 0xE6000-0xEC000 (24KB)
  
- **RAM**: 256KB total
  - SoftDevice: 0x20000000-0x20003000 (12KB)
  - Application: 0x20003000-0x20040000 (244KB)

## HubMsg Protocol

The dongle communicates with the PC using the HubMsg protocol (minicbor-encoded):

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

### Nordic S140 SoftDevice

Uses Nordic's officially qualified S140 SoftDevice v7.x with:
- **Central role** with support for up to 7 simultaneous connections
- **Scanning** for devices with configurable parameters
- **GATT Client** for reading/writing characteristics
- **Qualified stack** for easier regulatory certification

### SoftDevice Configuration

```rust
sd::Config {
    conn_gap: Some(ble_gap_conn_cfg_t {
        conn_count: 7,  // Up to 7 simultaneous connections
        event_length: 24,
    }),
    conn_gatt: Some(ble_gatt_conn_cfg_t {
        att_mtu: 256,  // 256-byte MTU for larger packets
    }),
    gap_role_count: Some(ble_gap_cfg_role_count_t {
        central_role_count: 7,  // Central role for 7 devices
        ...
    }),
    ...
}
```

### Connection Flow

1. Dongle scans for devices matching configured UUIDs
2. Attempts to connect to each discovered device
3. Creates GATT client for each connection
4. Discovers ATS data characteristics
5. Subscribes to notifications for incoming data
6. Routes packets between USB and BLE connections

## Current Status

### ✅ Implemented
- [x] USB device with HubMsg protocol
- [x] S140 SoftDevice initialization and configuration
- [x] BLE central role with scanning
- [x] Multi-device connection management (up to 7)
- [x] GATT client framework
- [x] Device UUID storage system
- [x] Bidirectional data routing framework
- [x] Memory layout for SoftDevice compatibility

### 🚧 TODO
- [ ] GATT service/characteristic discovery for ATS devices
- [ ] Actual data read/write to characteristics
- [ ] Persistent storage for device UUIDs (flash)
- [ ] Add `AddDevice`/`RemoveDevice` to HubMsg protocol
- [ ] BLE bonding and pairing with security
- [ ] Reconnection logic for dropped connections
- [ ] LED status indicators

## Regulatory Qualification

Using Nordic's S140 SoftDevice provides several advantages for qualification:
- **Pre-qualified BLE stack**: S140 is already qualified by Nordic
- **Reduced certification effort**: Focus on application layer, not BLE stack
- **Known compliance**: S140 has passed FCC, CE, IC, and other certifications
- **Official support**: Nordic provides documentation and support for qualification

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

1. Use `probe-rs` with RTT for logging
2. SoftDevice errors are logged via defmt
3. USB communication can be monitored with Wireshark + USBPcap

### Testing

1. Flash S140 SoftDevice to dongle
2. Flash application firmware
3. Configure device UUIDs (currently hardcoded, needs protocol extension)
4. Connect to PC via USB
5. Start vm-fw devices
6. Use protodonge-rs Python/Rust client to communicate

## Troubleshooting

### SoftDevice Assert

If you see "SoftDevice assert", common causes:
- Incorrect memory layout (check flash/RAM regions)
- API called from wrong interrupt priority
- Invalid BLE parameters

### Connection Issues

- Ensure vm-fw devices are advertising
- Check device UUIDs match configuration
- Verify SoftDevice is properly flashed
- Check that devices are using expected BLE addresses

### USB Not Detected

- Verify VID/PID (0x1915:0x5210)
- Check USB cable supports data (not charge-only)
- Try different USB port
- Check WinUSB driver installed (Windows)

## See Also

- [PROTOCOL_EXTENSION.md](PROTOCOL_EXTENSION.md) - Proposed device management protocol additions
- [protodonge-rs](https://github.com/odysseyarm/protodonge-rs) - HubMsg protocol definition
- [nrf-softdevice](https://github.com/embassy-rs/nrf-softdevice) - Rust bindings for SoftDevice
- [S140 Documentation](https://infocenter.nordicsemi.com/topic/struct_nrf52/struct/s140.html) - Official Nordic docs
