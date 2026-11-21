# embassy-usb-dfu

An implementation of the USB DFU 1.1 protocol using embassy-boot. It has 2 components, and the feature you enable picks which one you get.

DFU protocol mode is the `dfu` feature. This is the transfer phase of the DFU protocol as described by the USB IF. It supports DFU_DNLOAD requests if marked by the user, and it resets the chip on its own once a DFU transaction has completed. DFU_GETSTATUS, DFU_GETSTATE, DFU_ABORT, and DFU_CLRSTATUS are answered with no user intervention.

DFU runtime mode is the `application` feature. With it your USB device exposes a DFU interface. That tells the host the device can DFU over USB, and it lets the host reset the device into its bootloader to complete a DFU operation. Runtime mode supports DFU_GETSTATUS and DFU_DETACH. When the device sees a detach/reset as described by the standard, it writes a new DFU magic number into the bootloader state in flash and resets the system.

`Control` writes through one `FirmwareWriter`. `Control::new_single_bank` uses
embassy-boot's `BlockingFirmwareUpdater` for a scratch-swap layout; a dual-bank
bootloader passes its own writer to `Control::new`, which also receives net-core
and stage2 images by `DfuTarget`.

## Verification

Supported ATS Lite and marker bootloaders enable `signed-dfu`; ATS Lite selects
`signed-dfu-oberon` for CC312 Ed25519 and Oberon hashing. Downloads start with
the 132-byte `release-signing` header: the existing 67-byte signed message,
64-byte signature and one reserved zero byte. The original image follows,
without transport padding. The header is never stored in an application bank.

The receiver authenticates the header before flash writes, checks block order
and signing role, and verifies exact image length and SHA-256 before activation.
Application, net-core and stage2 targets use separate component IDs. Failed
downloads enter DFU error state; `DFU_ABORT` or `DFU_CLRSTATUS` permits a retry.
Flash-word padding is not part of the signed image.

Bootloader 1.0.0 requires this envelope. Older bootloaders accept raw images;
Desktop selects the format from the running bootloader version. Create a
standalone download with `cargo xtask firmware sign <component> <image>
--version <version> --output <image.dfu>`, using `ODYSSEY_SIGNER`, then pass
that `.dfu` file to `dfu-util -D`. A raw `.bin` is rejected by enforcing bootloaders.

This authenticates DFU installation, not every boot. It does not add protected
secure boot, general anti-rollback counters or SWD restrictions.
