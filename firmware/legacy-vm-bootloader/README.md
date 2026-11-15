# Bootloader for nRF

The bootloader talks to flash through `embassy-boot`.

# Usage

Flash the bootloader

```
cargo flash --features embassy-nrf/nrf5340 --release --chip nRF5340_xxAA
```
