//! IMU initialization and handling for lite1 board
//!
//! Uses ICM-42688-P via SPI on SERIAL0

use embassy_nrf::{
    Peri,
    gpio::{AnyPin, Input, Level, Output, OutputDrive, Pull},
    gpiote::{self, OutputChannelPolarity},
    peripherals::SERIAL0,
    ppi::{self, Ppi},
    spim::{self, Spim},
    timer::{self, Timer},
};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use icm426xx::{ICM42688, Ready};

use crate::Irqs;

/// IMU driver type
pub type Imu = ICM42688<ExclusiveDevice<Spim<'static>, Output<'static>, Delay>, Ready>;

/// IMU interrupt wrapper
pub struct ImuInterrupt {
    pub int1: Input<'static>,
}

impl ImuInterrupt {
    pub async fn wait(&mut self) {
        self.int1.wait_for_low().await;
    }
}

const VALID_ODRS_ICM42688: &[(u16, icm426xx::config::AccelOdr)] = {
    use icm426xx::config::AccelOdr;
    &[
        (2, AccelOdr::_1_5625Hz),
        (3, AccelOdr::_3_125Hz),
        (6, AccelOdr::_6_25Hz),
        (13, AccelOdr::_12_5Hz),
        (25, AccelOdr::_25Hz),
        (50, AccelOdr::_50Hz),
        (100, AccelOdr::_100Hz),
        (200, AccelOdr::_200Hz),
        (500, AccelOdr::_500Hz),
        (1000, AccelOdr::_1kHz),
        (2000, AccelOdr::_2kHz),
        (4000, AccelOdr::_4kHz),
        (8000, AccelOdr::_8kHz),
        (16000, AccelOdr::_16kHz),
        (32000, AccelOdr::_32kHz),
    ]
};

fn nearest_odr_index(hz: u16) -> usize {
    let mut best = 0;
    for (i, &(val, _)) in VALID_ODRS_ICM42688.iter().enumerate() {
        let curr_diff = (hz as i32 - VALID_ODRS_ICM42688[best].0 as i32).unsigned_abs();
        let new_diff = (hz as i32 - val as i32).unsigned_abs();
        if new_diff < curr_diff {
            best = i;
        }
    }
    best
}

fn round_odr_icm42688(hz: u16) -> icm426xx::config::AccelOdr {
    VALID_ODRS_ICM42688[nearest_odr_index(hz)].1
}

/// Round accelerometer ODR to nearest valid value
pub fn round_accel_odr_hz(hz: u16) -> u16 {
    VALID_ODRS_ICM42688[nearest_odr_index(hz)].0
}

/// Initialize the ICM-42688-P IMU on SERIAL0
#[allow(clippy::too_many_arguments)]
pub async fn init<T: timer::Instance, C: gpiote::Channel>(
    spim_instance: Peri<'static, SERIAL0>,
    cs: Peri<'static, AnyPin>,
    sck: Peri<'static, AnyPin>,
    miso: Peri<'static, AnyPin>,
    mosi: Peri<'static, AnyPin>,
    int1: Peri<'static, AnyPin>,
    clkin: Peri<'static, AnyPin>,
    clkin_timer_instance: Peri<'static, T>,
    clkin_ppi_ch: Peri<'static, ppi::AnyConfigurableChannel>,
    clkin_gpiote_ch: Peri<'static, C>,
    accel_odr: u16,
) -> (Imu, ImuInterrupt) {
    let int1 = Input::new(int1, Pull::None);
    let cs = Output::new(cs, Level::High, OutputDrive::Standard);
    let clkin_ch = gpiote::OutputChannel::new(
        clkin_gpiote_ch,
        clkin,
        Level::Low,
        OutputDrive::Standard,
        OutputChannelPolarity::Toggle,
    );

    // Generate a 32 kHz clock on CLKIN
    let timer = Timer::new(clkin_timer_instance);
    timer.set_frequency(timer::Frequency::F8MHz);
    let cc = timer.cc(0);
    cc.write(125);
    cc.short_compare_clear();
    let mut ppi_ch = Ppi::new_one_to_one(clkin_ppi_ch, cc.event_compare(), clkin_ch.task_out());
    ppi_ch.enable();
    timer.start();
    // Let the timer run forever
    timer.persist();
    ppi_ch.persist();
    clkin_ch.persist();

    let mut spim_config = spim::Config::default();
    spim_config.frequency = spim::Frequency::M8;
    spim_config.mode = spim::MODE_3;
    spim_config.bit_order = spim::BitOrder::MSB_FIRST;
    let spim = Spim::new(spim_instance, Irqs, sck, miso, mosi, spim_config);

    let Ok(spi_device) = ExclusiveDevice::new(spim, cs, Delay);
    let imu = ICM42688::new(spi_device);
    let mut imu_config = icm426xx::Config::default();
    {
        use icm426xx::config::{AccelMode, Drive, GyroOdr, Pin9Function, Polarity};
        imu_config.gyro.odr = GyroOdr::_100Hz;
        imu_config.accel.odr = round_odr_icm42688(accel_odr);
        imu_config.accel.mode = AccelMode::LowNoise;
        imu_config.int1.drive = Drive::PushPull;
        imu_config.int1.polarity = Polarity::ActiveLow;
        imu_config.pin9.function = Pin9Function::CLKIN;
        imu_config.fifo_watermark = 1;
    }
    let imu = imu.initialize(Delay, imu_config).await.unwrap();

    defmt::info!("ICM42688 initialized");

    (imu, ImuInterrupt { int1 })
}
