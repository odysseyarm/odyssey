// VM Platform trait implementation

use crate::imu::{VmImu, VmImuInterrupt};
use crate::l2cap::VmL2capChannels;

pub struct VmPlatform;

impl common::platform::Platform for VmPlatform {
    type Imu = VmImu<2048>; // 2048 byte burst buffer
    type ImuInterrupt = VmImuInterrupt;
    type L2capChannels = VmL2capChannels;
    type Timer = embassy_nrf::peripherals::TIMER1;

    const NAME: &'static str = "VM (nRF52840)";

    // BMI270 with 16g range: 2048 LSB/g
    const ACC_LSB_PER_G: f32 = 2048.0;

    // BMI270 with 500 dps range: 65.536 LSB/(deg/s)
    const GYRO_LSB_PER_DPS: f32 = 65.536;

    // BMI270 axis transforms: flip X and Y, keep Z
    fn transform_accel(raw: [i32; 3]) -> [i32; 3] {
        [-raw[0], -raw[1], raw[2]]
    }

    fn transform_gyro(raw: [i32; 3]) -> [i32; 3] {
        [-raw[0], -raw[1], raw[2]]
    }

    fn round_accel_odr(hz: u16) -> u16 {
        crate::imu::round_accel_odr_hz(hz)
    }

    fn device_id() -> [u8; 8] {
        crate::utils::device_id()
    }
}
