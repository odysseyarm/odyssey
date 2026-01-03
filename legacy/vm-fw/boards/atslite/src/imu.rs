use core::future::Future;

use embassy_nrf::gpio::{AnyPin, Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::gpiote::{self, OutputChannelPolarity};
use embassy_nrf::peripherals::SPIM4;
use embassy_nrf::ppi::{self, Ppi};
use embassy_nrf::timer::{self, Timer};
use embassy_nrf::{Peri, interrupt, spim};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use icm426xx::{ICM42688, Ready};

pub struct AtsliteImu {
    inner: ICM42688<ExclusiveDevice<spim::Spim<'static>, Output<'static>, Delay>, Ready>,
    // Accumulated timestamp in microseconds
    ts_micros: u32,
}

pub struct AtsliteImuInterrupt {
    int1: Input<'static>,
}

#[derive(Debug)]
pub struct ImuError;

// Implement the Imu trait from common::platform
impl common::platform::Imu for AtsliteImu {
    type Error = ImuError;
    type Interrupt = AtsliteImuInterrupt;

    fn read_data(
        &mut self,
    ) -> impl Future<Output = Result<common::platform::ImuData, Self::Error>> {
        async move {
            // Read from FIFO - buffer for 521 u32 words as in the old implementation
            let mut buffer = [0u32; 521];
            let _items = self
                .inner
                .read_fifo(&mut buffer)
                .await
                .map_err(|_| ImuError)?;

            // Parse the first FIFO packet (skip 4 bytes header, read 20-byte packet)
            let pkt = bytemuck::from_bytes::<icm426xx::fifo::FifoPacket4>(
                &bytemuck::cast_slice(&buffer)[4..24],
            );

            // Read raw sensor data - axis transforms done in common lib via Platform trait
            // Keep as i32 to avoid truncation
            let ax = pkt.accel_data_x();
            let ay = pkt.accel_data_y();
            let az = pkt.accel_data_z();

            let gx = pkt.gyro_data_x();
            let gy = pkt.gyro_data_y();
            let gz = pkt.gyro_data_z();

            // Accumulate timestamp delta from FIFO (CLKIN ticks at 32 kHz)
            // The timestamp is the delta since last sample, convert to microseconds: ticks * 1000 / 32
            let timestamp_delta = 1000 * (pkt.timestamp() as u32) / 32;
            self.ts_micros = self.ts_micros.wrapping_add(timestamp_delta);

            Ok(common::platform::ImuData {
                accel: [ax, ay, az],
                gyro: [gx, gy, gz],
                timestamp_micros: self.ts_micros,
            })
        }
    }
}

// Implement the ImuInterrupt trait
impl common::platform::ImuInterrupt for AtsliteImuInterrupt {
    fn wait(&mut self) -> impl Future<Output = ()> {
        self.int1.wait_for_low()
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn init<T: timer::Instance, C: gpiote::Channel>(
    spim_instance: Peri<'static, SPIM4>,
    cs: Peri<'static, AnyPin>,
    sck: Peri<'static, AnyPin>,
    miso: Peri<'static, AnyPin>,
    mosi: Peri<'static, AnyPin>,
    int1: Peri<'static, AnyPin>,
    clkin: Peri<'static, AnyPin>,
    clkin_timer_instance: Peri<'static, T>,
    clkin_ppi_ch: Peri<'static, ppi::AnyConfigurableChannel>,
    clkin_gpiote_ch: Peri<'static, C>,
) -> (AtsliteImu, AtsliteImuInterrupt)
where
    crate::Irqs: interrupt::typelevel::Binding<
            <SPIM4 as spim::Instance>::Interrupt,
            spim::InterruptHandler<SPIM4>,
        >,
{
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
    let spim = spim::Spim::new(spim_instance, crate::Irqs, sck, miso, mosi, spim_config);

    let Ok(spi_device) = ExclusiveDevice::new(spim, cs, Delay);
    let imu = ICM42688::new(spi_device);
    let mut imu_config = icm426xx::Config::default();
    {
        use icm426xx::config::{AccelMode, AccelOdr, Drive, GyroOdr, Pin9Function, Polarity};
        imu_config.gyro.odr = GyroOdr::_100Hz;
        imu_config.accel.odr = AccelOdr::_100Hz;
        imu_config.accel.mode = AccelMode::LowNoise;
        imu_config.int1.drive = Drive::PushPull;
        imu_config.int1.polarity = Polarity::ActiveLow;
        imu_config.pin9.function = Pin9Function::CLKIN;
        imu_config.fifo_watermark = 1;
    }
    let imu = imu.initialize(Delay, imu_config).await.unwrap();

    defmt::info!("ICM42688 initialized");

    (
        AtsliteImu {
            inner: imu,
            ts_micros: 0,
        },
        AtsliteImuInterrupt { int1 },
    )
}
