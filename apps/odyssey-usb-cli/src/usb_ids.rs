//! USB identifiers of the direct (lite/vm) devices this tool talks to.

/// Nordic Semiconductor's USB vendor id.
const NORDIC_VENDOR_ID: u16 = 0x1915;
/// Product ids a direct device may enumerate with.
const VM_PRODUCT_IDS: [u16; 3] = [0x520F, 0x5210, 0x5211];

pub(crate) fn is_vm_device(vendor_id: u16, product_id: u16) -> bool {
    vendor_id == NORDIC_VENDOR_ID && VM_PRODUCT_IDS.contains(&product_id)
}
