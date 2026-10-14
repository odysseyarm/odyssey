# Bootloader for nRF

The bootloader uses `embassy-boot` to interact with the flash.

# Usage

Flash the bootloader

```
cargo flash --features embassy-nrf/nrf5340 --release --chip nRF5340_xxAA
```
