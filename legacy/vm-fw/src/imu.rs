use core::convert::Infallible;

use bmi2::{bmi2_async::Bmi2, config, interface::SpiInterface, types::{Burst, Error}};
use embassy_nrf::{
    Peri, gpio::{AnyPin, Input, Output, Pull}, interrupt::typelevel::Binding, spim::{self, Spim},
};
use embassy_time::Delay;
use embedded_hal_bus::spi::DeviceError;

use crate::{pins, utils::make_spi_dev};

pub type Imu<const N: usize> = Bmi2<SpiInterface<embedded_hal_bus::spi::ExclusiveDevice<Spim<'static>, Output<'static>, Delay>>, Delay, N>;

pub struct ImuInterrupt {
    irq: Input<'static>,
}

impl ImuInterrupt {
    pub fn wait(&mut self) -> impl Future<Output = ()> {
        self.irq.wait_for_low()
    }
}

pub async fn init<SPI, IRQ, const N: usize, const MAX: u16>(
    spi: Peri<'static, SPI>,
    irqs: IRQ,
    pins: pins::Spi,
    irq: Peri<'static, AnyPin>,
) -> Result<(Imu<N>, ImuInterrupt), Error<DeviceError<spim::Error, Infallible>>>
where
    SPI: spim::Instance,
    IRQ: Binding<<SPI as spim::Instance>::Interrupt, spim::InterruptHandler<SPI>> + 'static,
{
    let irq = Input::new(irq, Pull::None);

    let mut spim_config = spim::Config::default();
    spim_config.frequency = spim::Frequency::M8;
    spim_config.mode = spim::MODE_0;
    spim_config.bit_order = spim::BitOrder::MSB_FIRST;
    
    let spi_device = make_spi_dev(spi, irqs, pins, spim_config);

    let mut imu: Bmi2<_, _, N> = Bmi2::new_spi(spi_device, Delay, Burst::new(MAX));

    let chip_id = imu.get_chip_id().await.unwrap();
    defmt::info!("chip id: {}", chip_id);

    imu.init(&config::BMI270_CONFIG_FILE).await?;
    let acc_conf = {
        use bmi2::types::{AccConf, AccBwp, Odr, PerfMode};
        AccConf {
            odr: Odr::Odr100,
            bwp: AccBwp::Osr4Avg1,
            filter_perf: PerfMode::Perf,
        }
    };
    let gyr_conf = {
        use bmi2::types::{GyrConf, GyrBwp, Odr, PerfMode};
        GyrConf {
            odr: Odr::Odr100,
            bwp: GyrBwp::Osr4,
            filter_perf: PerfMode::Perf,
            noise_perf: PerfMode::Perf,
        }
    };
    imu.set_acc_conf(acc_conf).await?;
    imu.set_gyr_conf(gyr_conf).await?;
    
    Ok((imu, ImuInterrupt { irq }))
}
