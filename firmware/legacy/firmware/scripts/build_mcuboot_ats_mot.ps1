# Enable strict error handling and verbose output
$ErrorActionPreference = "Stop"
$VerbosePreference = "Continue"

# Set default values for variables
$TARGET = if ($env:TARGET) { $env:TARGET } else { "thumbv7em-none-eabihf" }
$DONGER_HEX = if ($env:DONGER_HEX) { $env:DONGER_HEX } else { "target/$TARGET/release/donger.hex" }
$DONGER_SIGNED_HEX = if ($env:DONGER_SIGNED_HEX) { $env:DONGER_SIGNED_HEX } else { "target/$TARGET/release/donger.signed.hex" }
$DONGER_SIGNED_BIN = if ($env:DONGER_SIGNED_BIN) { $env:DONGER_SIGNED_BIN } else { "target/$TARGET/release/donger.signed.bin" }

# Run cargo build
cargo build --release --features="mcuboot,$FEATURES" --target="$TARGET"

# Convert the built firmware to Intel HEX format using llvm-objcopy
llvm-objcopy -O ihex "target/$TARGET/release/donger-firmware" "$DONGER_HEX"

# Create a signed image using imgtool
imgtool create --align 4 -v 0.0.1 --pad-header -H 512 -S 0x67000 -k "../../usb-vision-module-nordic/bootloader/mcuboot/root-ec-p256.pem" "$DONGER_HEX" "$DONGER_SIGNED_HEX"

# Convert the signed HEX to binary format using llvm-objcopy
llvm-objcopy -I ihex -O binary "$DONGER_SIGNED_HEX" "$DONGER_SIGNED_BIN"
