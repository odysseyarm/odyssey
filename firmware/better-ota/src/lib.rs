//! Device side of the radio firmware update (OTA): the transport-independent state machine that
//! turns request frames into staged flash images and, on `Commit`, into a boot-time activation.
//!
//! # End-to-end flow
//!
//! 1. **Host sender.** `crates/odyssey-hub-server/src/firmware/ota.rs` reads the status, picks the
//!    images that apply (net core first, then application), and sends `Begin`, `Data` chunks and
//!    `Commit` through its `OtaTransport`. Chunks are sized to the link: 92 bytes on the vendor
//!    packet link, 12 bytes on the marker GATT link. After `Commit` it waits for the device to
//!    reconnect with the new versions.
//! 2. **Wire format.** Frames and the 22-byte status are defined in
//!    `crates/odyssey-protocol/src/ota.rs`. Every request gets one status reply.
//! 3. **Transport.** Three routes reach a device. All of them carry one frame per exchange and
//!    never overlap two exchanges:
//!    - Trackers (lite1, legacy-atslite): a vendor packet with tag `0xfe` over the BLE packet
//!      link. The hub side is `crates/odyssey-hub-server/src/device_tasks/handler/commands.rs`;
//!      the firmware side is the vendor branch in `firmware/atslite-common/src/object_mode.rs`
//!      (and `firmware/legacy-common/src/object_mode.rs`), which hands the frame to
//!      `OtaHandle::request` and sends the status back as the reply packet.
//!    - Marker board over the system Bluetooth stack: the hub writes the OTA GATT characteristic
//!      with a write-with-response, then reads it for the status
//!      (`crates/odyssey-hub-server/src/accessories/transport.rs`). The characteristic is served
//!      by `session` in `firmware/marker-control-board/src/ble.rs`.
//!    - Marker board through a dongle: the hub sends the mux commands `MarkerOta` and
//!      `MarkerOtaStatus`; `firmware/dongle-fw/src/ble/accessories.rs` relays them as the same
//!      GATT write and read.
//! 4. **[`Receiver`].** `firmware/better-ota/src/receiver.rs` decodes the frame, enforces order
//!    (`Begin`, contiguous `Data`, then `Commit`), checks the CRC-32 by reading the image back
//!    from flash, and asks the [`Slots`] backend to validate it. Any failure drops all staged
//!    images, so a commit only ever activates images that were verified in this session. With the
//!    `signed` feature the host also sends each image's 76-byte signature record (version and
//!    Ed25519 signature) as in-order `Signature` chunks after its last `Data`, sized to the
//!    link so a 20-byte GATT write carries them too, and `Commit` hashes each staged image
//!    from flash and checks the signature against `firmware/release-signing`'s trusted keys; the
//!    status reports this as `signature_required`, which makes the host send the signature.
//! 5. **[`Slots`] backends.** One per board, each owning the flash layout:
//!    - `FlashSlots` in `firmware/atslite-common/src/ota.rs` (lite1 and legacy-atslite). It writes
//!      the application into the inactive bank and the net core into a staging region, and its
//!      `OtaHandle`/`run` pair is a task that owns the `Receiver` so the transports can stay
//!      stateless.
//!    - `MarkerSlots` in `firmware/marker-control-board/src/ble.rs`. It writes the application
//!      into the DFU partition and has no net core.
//! 6. **Commit.** `Slots::commit` records the activation in flash. On trackers
//!    `better_boot::write_ota_state` (`firmware/better-boot/src/lib.rs`) rewrites the boot-state
//!    page: the pending application slot, the net core record the bootloader installs from, and
//!    the boot magic. The marker board instead asks the task
//!    in `firmware/marker-control-board/src/main.rs` to call embassy-boot's `mark_updated`.
//! 7. **Reset and bootloader.** `Receiver::handle` returns [`Action::Reset`] only after a
//!    successful commit. The transport sends the status reply first and then resets the device.
//!    The bootloader (`firmware/lite1-bootloader`, `firmware/legacy-atslite-bootloader`,
//!    `firmware/marker-control-board-bootloader`) validates the pending image and boots it. The
//!    tracker bootloaders (`better_boot::select_boot_slot`) and the marker bootloader (embassy-boot)
//!    fall back to the previous image if the new one never confirms the boot.
//!
//! The crate names no flash driver, executor or transport, so the receiver tests run on the host
//! and the hub-server tests drive the real `Receiver` against the real sender.

#![no_std]
#[cfg(test)]
extern crate std;

mod receiver;

pub use receiver::{Action, Receiver, Slots};
