//! Typed accessors for the image sensors' registers, one line per register.
//! Each `(bank, addresses)` pair is the sensor datasheet's register map. A
//! multi-byte value is stored little-endian across the listed addresses.

use super::VmDevice;
use crate::packets::vm::Port;

macro_rules! read_register_spec {
    ($name:ident : $ty:ty = $bank:literal; [$($addr:literal),*]) => {
        /// # Errors
        /// Returns register transport or unexpected-reply errors.
        pub async fn $name(&self, port: Port) -> $crate::Result<$ty> {
            let mut bytes = <$ty>::to_le_bytes(0);
            for (byte, addr) in ::std::iter::zip(&mut bytes, [$($addr),*]) {
                *byte = self.read_register(port, $bank, addr).await?;
            }
            Ok(<$ty>::from_le_bytes(bytes))
        }
    }
}

macro_rules! write_register_spec {
    ($name:ident : $ty:ty = $bank:literal; [$($addr:literal),*]) => {
        /// # Errors
        /// Returns register transport errors.
        pub async fn $name(&self, port: Port, value: $ty) -> $crate::Result<()> {
            let bytes = <$ty>::to_le_bytes(value);
            for (byte, addr) in ::std::iter::zip(bytes, [$($addr),*]) {
                self.write_register(port, $bank, addr, byte).await?;
            }
            Ok(())
        }
    }
}

impl VmDevice {
    // PAJ sensor registers
    read_register_spec!(product_id: u16 = 0x00; [0x02, 0x03]);
    read_register_spec!(resolution_x: u16 = 0x0c; [0x60, 0x61]);
    read_register_spec!(resolution_y: u16 = 0x0c; [0x62, 0x63]);
    write_register_spec!(set_resolution_x: u16 = 0x0c; [0x60, 0x61]);
    write_register_spec!(set_resolution_y: u16 = 0x0c; [0x62, 0x63]);
    read_register_spec!(gain_1: u8 = 0x01; [0x05]); // B_global
    read_register_spec!(gain_2: u8 = 0x01; [0x06]); // B_ggh
    write_register_spec!(set_gain_1: u8 = 0x0c; [0x0b]); // B_global
    write_register_spec!(set_gain_2: u8 = 0x0c; [0x0c]); // B_ggh
    read_register_spec!(exposure_time: u16 = 0x01; [0x0e, 0x0f]);
    write_register_spec!(set_exposure_time: u16 = 0x0c; [0x0f, 0x10]);
    read_register_spec!(brightness_threshold: u8 = 0x0c; [0x47]);
    write_register_spec!(set_brightness_threshold: u8 = 0x0c; [0x47]);
    read_register_spec!(noise_threshold: u8 = 0x00; [0x0f]);
    write_register_spec!(set_noise_threshold: u8 = 0x00; [0x0f]);
    read_register_spec!(area_threshold_max: u16 = 0x00; [0x0b, 0x0c]);
    write_register_spec!(set_area_threshold_max: u16 = 0x00; [0x0b, 0x0c]);
    read_register_spec!(area_threshold_min: u8 = 0x0c; [0x46]);
    write_register_spec!(set_area_threshold_min: u8 = 0x0c; [0x46]);
    read_register_spec!(operation_mode: u8 = 0x00; [0x12]);
    write_register_spec!(set_operation_mode: u8 = 0x00; [0x12]);
    read_register_spec!(max_object_cnt: u8 = 0x00; [0x19]);
    write_register_spec!(set_max_object_cnt: u8 = 0x00; [0x19]);
    read_register_spec!(frame_subtraction: u8 = 0x00; [0x28]);
    write_register_spec!(set_frame_subtraction: u8 = 0x00; [0x28]);
    read_register_spec!(frame_period: u32 = 0x0c; [0x07, 0x08, 0x09]);
    write_register_spec!(set_frame_period: u32 = 0x0c; [0x07, 0x08, 0x09]);
    write_register_spec!(set_bank1_sync_updated: u8 = 0x01; [0x01]);
    write_register_spec!(set_bank0_sync_updated: u8 = 0x00; [0x01]);

    // PAG7661QN registers
    read_register_spec!(pag_chip_id: u16 = 0x00; [0x00, 0x01]);
    read_register_spec!(pag_fps: u16 = 0x00; [0x13]);
    write_register_spec!(set_pag_fps: u8 = 0x00; [0x13]);
    read_register_spec!(pag_exposure: u8 = 0x00; [0x66]);
    write_register_spec!(set_pag_exposure: u8 = 0x00; [0x66]);
    read_register_spec!(pag_gain: u8 = 0x00; [0x67]);
    write_register_spec!(set_pag_gain: u8 = 0x00; [0x67]);
    read_register_spec!(pag_area_lower: u16 = 0x00; [0x68, 0x69]);
    write_register_spec!(set_pag_area_lower: u16 = 0x00; [0x68, 0x69]);
    read_register_spec!(pag_area_upper: u16 = 0x00; [0x6A, 0x6B]);
    write_register_spec!(set_pag_area_upper: u16 = 0x00; [0x6A, 0x6B]);
    read_register_spec!(pag_light_threshold: u8 = 0x00; [0x6C]);
    write_register_spec!(set_pag_light_threshold: u8 = 0x00; [0x6C]);

    // PAG7665QN registers (different addresses from PAG7661QN)
    read_register_spec!(pag7665_exposure: u8 = 0x00; [0x67]);
    write_register_spec!(set_pag7665_exposure: u8 = 0x00; [0x67]);
    read_register_spec!(pag7665_gain: u8 = 0x00; [0x68]);
    write_register_spec!(set_pag7665_gain: u8 = 0x00; [0x68]);
    read_register_spec!(pag7665_area_lower: u16 = 0x00; [0x6E, 0x6F]);
    write_register_spec!(set_pag7665_area_lower: u16 = 0x00; [0x6E, 0x6F]);
    read_register_spec!(pag7665_area_upper: u16 = 0x00; [0x70, 0x71]);
    write_register_spec!(set_pag7665_area_upper: u16 = 0x00; [0x70, 0x71]);
    read_register_spec!(pag7665_light_threshold: u8 = 0x00; [0x6D]);
    write_register_spec!(set_pag7665_light_threshold: u8 = 0x00; [0x6D]);

    // PAG7665QN circle detection: R ratio bounds, then K ratio bounds.
    read_register_spec!(pag_circle_r_min: u8 = 0x00; [0x23]);
    write_register_spec!(set_pag_circle_r_min: u8 = 0x00; [0x23]);
    read_register_spec!(pag_circle_r_max: u8 = 0x00; [0x24]);
    write_register_spec!(set_pag_circle_r_max: u8 = 0x00; [0x24]);
    read_register_spec!(pag_circle_k_min: u8 = 0x00; [0x77]);
    write_register_spec!(set_pag_circle_k_min: u8 = 0x00; [0x77]);
    read_register_spec!(pag_circle_k_max: u8 = 0x00; [0x78]);
    write_register_spec!(set_pag_circle_k_max: u8 = 0x00; [0x78]);
}
