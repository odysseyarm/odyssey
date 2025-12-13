// ATSlite Platform trait implementation

use crate::imu::{AtsliteImu, AtsliteImuInterrupt};
use crate::l2cap::AtsliteL2capChannels;

pub struct AtslitePlatform;

impl common::platform::Platform for AtslitePlatform {
    type Imu = AtsliteImu;
    type ImuInterrupt = AtsliteImuInterrupt;
    type L2capChannels = AtsliteL2capChannels;
    type Timer = embassy_nrf::peripherals::TIMER0;

    const NAME: &'static str = "ATSlite (nRF5340)";

    fn device_id() -> [u8; 8] {
        crate::utils::device_id()
    }
}
