#!/bin/bash

set -ex

TARGET="${TARGET:-thumbv8m.main-none-eabihf}"
DONGER_HEX="${DONGER_HEX:-target/"$TARGET"/release/donger.hex}"
DONGER_SIGNED_HEX="${DONGER_SIGNED_HEX:-target/"$TARGET"/release/donger.signed.hex}"
DONGER_SIGNED_BIN="${DONGER_SIGNED_BIN:-target/"$TARGET"/release/donger.signed.bin}"

cargo build --release --features="mcuboot,$FEATURES" --no-default-features --target="$TARGET"
objcopy -O ihex target/"$TARGET"/release/donger-firmware "$DONGER_HEX"
imgtool create --align 4 -v 0.0.1 --pad-header -H 512 -S 0x40000 -k ../../usb-vision-module-nordic/bootloader/mcuboot/root-rsa-2048.pem "$DONGER_HEX" "$DONGER_SIGNED_HEX"
objcopy -I ihex -O binary "$DONGER_SIGNED_HEX" "$DONGER_SIGNED_BIN"
