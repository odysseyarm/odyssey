# Dongle Firmware

nRF52840 USB dongle firmware acting as BLE-to-USB mux for three trackers plus three supported accessories. Accessory slots never replace tracker slots.

## Features

- **USB Communication**: VID 0x1915, PID 0x5212
- **Mux Protocol (bulk)**: Postcard-encoded MuxMsg over USB bulk endpoints
- **Control Protocol (EP0)**: USB control endpoint for pairing and bond management
- **BLE Central**: Six connections using trouble-host with nrf-sdc: three tracker slots plus three accessory slots
- **Device Management**: Pairing mode, bond storage, and device connection management
- **L2CAP CoC**: Credit-based flow control for efficient data transfer
- **Accessories**: Marker Control Board, DryFireMag, and MantisX; correlated USB completions, acknowledged marker writes, and raw shot notifications

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

## Updating in Odyssey Desktop

In Desktop 0.7.5 or newer, open **Devices → Update dongle**. Desktop downloads
and verifies both signed packages and selects the one the bootloader accepts;
customers do not need `nrfutil`, a vendor choice, or a manually downloaded ZIP.
When prompted, press the dongle's reset/DFU button. Keep it in the same USB port
until Desktop confirms the installed version. Trackers and accessories
disconnect during the update.

A dongle already in Nordic bootloader mode appears as **Recover dongle**, including
after restarting Desktop. If an update fails, leave it in that USB port and retry;
press reset/DFU again if needed.

Signed packages from 0.1.14 pad the application to the region in
`memory-dongle.kdl`. Desktop queries the bootloader's existing image layout
before erase and refuses a package whose staging could overwrite saved bonds.
Unsupported layouts stop before flashing. An interrupted single-bank update
requires bootloader recovery, not a chip erase.

## Building

### Prerequisites

- Rust nightly toolchain
- `nrfutil` with the `nrf5sdk-tools` extension (`nrfutil install nrf5sdk-tools`) for the DFU package and serial DFU
- nRF52840 dongle with its maker's Nordic secure DFU bootloader: Fanstel's, or the Laird/Ezurio BL654 USB (451-00004) default. Each only accepts a package signed with its own key, so pick `--vendor` to match.

### Build and Flash

Run from the repository root (`cargo xtask dongle --help` lists every command):

```bash
# Release artifacts (.hex, .uf2, signed -dfu-fanstel.zip and -dfu-laird.zip) in firmware/dongle-fw/dist
cargo xtask dongle dist

# Build the DFU package and flash it over serial (put dongle in bootloader mode first)
cargo xtask dongle flash --vendor fanstel --port COM78

# Print the defmt log from the dongle's serial port
cargo xtask dongle attach --port COM79
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
  - Nordic MBR: 0x00000–0x01000 (4 KiB)
  - Application: 0x01000–0xCC000
  - Settings: 0xCC000–0xDE000 (72 KiB), separate tracker/accessory records
  - Bootloader begins at 0xE0000

- **RAM**: 256 KiB total; first eight bytes reserved by bootloader
  - SDC reservation: 72,328 bytes, six centrals, 18 TX and 18 RX buffers. Smaller allocations fail controller initialization before USB command tasks start.
  - trouble-host: six connections and 24 L2CAP channels; existing 64-packet pool
  - Three tracker task instances plus three accessory GATT session task instances
  - Linked release build: 90.1% of RAM and 50% of flash used (416,704 B flash, 236,184 B RAM). Hardware stress run remains required before release.

## Mux Protocol (bulk)

The dongle communicates with the PC over USB bulk endpoints using the MuxMsg protocol (postcard-encoded):

### Messages from Host to Dongle:
- `RequestDevices`: Get list of configured device UUIDs
- `SendTo(SendTo)`: Send packet to specific device
- `ReadVersion()`: Get firmware and protocol version
- `AccessoryRequest { id, command }`: Capabilities, discovery reporting start/stop, Connect/enroll, Disconnect, MarkerDemand, and MarkerMode

### Messages from Dongle to Host:
- `DevicesSnapshot(Vec<Uuid>)`: Connected trackers only (up to three)
- `DevicePacket(DevicePacket)`: Packet received from a device
- `ReadVersionResponse(Version)`: Protocol and firmware version information
- `AccessoryCompletion { id, result }`: Correlated accepted BLE operation or explicit error, never mere queue acceptance
- `AccessoryEvent`: Discovery, marker pairing-window status, connection/disconnection, raw notifications, and errors

Accessory addresses use canonical most-significant-byte-first order, matching native host Bluetooth addresses. Tracker UUID byte order stays unchanged. Accessory commands require mux protocol 0.3.0 or newer; `MarkerMode` requires **0.4.0** (firmware **0.1.13+**). The host refuses to send it to older dongles because they cannot decode it and would never complete it.

### Accessory lifecycle and security

Existing background scanner owns controller scanning for tracker pairing/name caching; accessory Scan controls discovery reporting. Discovery cache holds 24 peers including BLE address kind. Name-only Mantis/DryFireMag advertisements are supported; GATT discovery verifies shared profile UUIDs before connection success.

Firmware **0.1.12+** forwards versioned marker service-data countdowns as
`PairingWindow` events. Zero means closed; valid open values are `1..=30`.
Unknown versions and malformed payloads do not report a window. Discovery and
name reports remain independent of pairing status. Older dongle firmware
provides no pairing status, so updated desktop enrollment stays disabled until
both the board and selected dongle support it. Mux protocol was **0.3.0** at that
point; existing accessory event indices are unchanged.

Three accessory sessions have independent bounded command queues. Connect completes after service/characteristic discovery and notification subscription, or marker security plus acknowledged OFF write. Disconnect cancels pending connect/security/GATT work and acknowledges observed link teardown; teardown timeout is an error. USB reset, deconfiguration, and physical disable stop all accessory sessions and invalidate pending USB replies. Tracker reconnect policy stays unchanged.

Markers use standard SMP and six separate persistent accessory bond slots (one per supported screen); tracker bond record format and three tracker bond slots stay unchanged. New ownership requires explicit host enrollment plus board's physical enrollment window. Dongle waits for successful flash write (including GC retry) and board's acknowledged OFF write before accepting enrollment. Background storage owner finishes in-flight persistence even if caller cancels. cfg-noodle sequential records preserve previous committed record across interrupted writes; failed persistence reports BondStorage, never success.

Only host MarkerDemand commands renew board lease. Queued demand older than two seconds is rejected; no dongle renewal timer exists. Board's six-second lease limits stale ON demand after host failure. Just Works enrollment does not provide MITM resistance inside physical pairing window.

`MarkerMode { address, mode }` (appended after `MarkerDemand`, so existing command indices are unchanged) writes the board's Mode characteristic (`Auto = 0`, `Off = 1`, `On = 2`) on an existing marker session and completes only after the GATT write acknowledgement. The characteristic is discovered when the command arrives, so connecting to boards without it is unaffected. The host uses it to reset a board a short button press left in On/Off back to Auto, then disconnects.


## BLE Implementation

### trouble-host with nrf-sdc

Uses Embassy's trouble-host BLE stack with Nordic's SoftDevice Controller (nrf-sdc):
- **Central role** with three tracker and three accessory connections
- **L2CAP CoC** (Connection-oriented Channels) for efficient data transfer
- **Pairing and bonding** with persistent bond storage
- **Connection management** with automatic reconnection

### Connection Flow

1. Device enters pairing mode when requested via USB control endpoint
2. Scans for advertising peripherals and captures new devices
3. Bonds are persisted to flash storage
4. On startup, automatically reconnects bonded trackers; accessories connect only on host request
5. L2CAP CoC channels established for bidirectional data transfer
6. Routes packets between USB bulk endpoints and BLE L2CAP channels

## Current Status

### ✅ Implemented
- [x] USB device with MuxMsg (bulk) protocol
- [x] USB control endpoint (EP0) for device management
- [x] BLE central role with trouble-host stack
- [x] Independent tracker/accessory connection budgets (three each, six total)
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
# Build, flash and run on a DK with RTT logging
cargo xtask dongle flash-dk

# Attach to a running DK with probe-rs
cargo xtask dongle attach-dk
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
