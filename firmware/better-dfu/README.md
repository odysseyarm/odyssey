# embassy-usb-dfu

An implementation of the USB DFU 1.1 protocol using embassy-boot. It has 2 components, and the feature you enable picks which one you get.

DFU protocol mode is the `dfu` feature. This is the transfer phase of the DFU protocol as described by the USB IF. It supports DFU_DNLOAD requests if marked by the user, and it resets the chip on its own once a DFU transaction has completed. DFU_GETSTATUS, DFU_GETSTATE, DFU_ABORT, and DFU_CLRSTATUS are answered with no user intervention.

DFU runtime mode is the `application` feature. With it your USB device exposes a DFU interface. That tells the host the device can DFU over USB, and it lets the host reset the device into its bootloader to complete a DFU operation. Runtime mode supports DFU_GETSTATUS and DFU_DETACH. When the device sees a detach/reset as described by the standard, it writes a new DFU magic number into the bootloader state in flash and resets the system.

## Verification

Embassy-boot can verify that an update binary has been correctly signed using ed25519, as described in https://embassy.dev/book/#_verification. The linked procedure concatenates the signature to the end of the update binary, but embassy-boot does not force this and is flexible about how the signature for a binary is distributed. embassy-usb-dfu is stricter. The current implementation assumes the signature is 64 bytes long and concatenated to the end of the update binary, since that is the simplest way to make it work with the usb-dfu mechanism.

To enable verification, enable the `ed25519-dalek` or the `ed25519-salty` feature. `ed25519-salty` is the recommended one.
