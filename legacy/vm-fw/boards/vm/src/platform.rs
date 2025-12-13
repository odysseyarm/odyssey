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

    fn device_id() -> [u8; 8] {
        crate::utils::device_id()
    }
}
