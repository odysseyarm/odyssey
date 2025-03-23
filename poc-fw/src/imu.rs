use embassy_nrf::{
    Peripheral,
    gpio::{AnyPin, Input, Level, Output, OutputDrive, Pull},
    gpiote::{self, OutputChannelPolarity},
    interrupt,
    peripherals::SERIAL1,
    ppi::{self, Ppi},
    spim,
    timer::{self, Timer},
};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use icm426xx::{ICM42688, Ready};

pub type Imu =
    ICM42688<ExclusiveDevice<spim::Spim<'static, SERIAL1>, Output<'static>, Delay>, Ready>;

#[allow(clippy::too_many_arguments)]
pub async fn init(
    spim_instance: impl Peripheral<P = SERIAL1> + 'static,
    cs: AnyPin,
    sck: AnyPin,
    miso: AnyPin,
    mosi: AnyPin,
    int1: AnyPin,
    clkin: AnyPin,
    clkin_timer_instance: impl Peripheral<P: timer::Instance> + 'static,
    clkin_ppi_ch: ppi::AnyConfigurableChannel,
    clkin_gpiote_ch: gpiote::AnyChannel,
) -> (Imu, ImuInterrupt)
where
    crate::Irqs: interrupt::typelevel::Binding<
            <SERIAL1 as spim::Instance>::Interrupt,
            spim::InterruptHandler<SERIAL1>,
        >,
{
    let int1 = Input::new(int1, Pull::None);
    let clkin = Output::new(clkin, Level::Low, OutputDrive::Standard);
    let cs = Output::new(cs, Level::High, OutputDrive::Standard);
    let clkin_ch =
        gpiote::OutputChannel::new(clkin_gpiote_ch, clkin, OutputChannelPolarity::Toggle);

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
    core::mem::forget(timer);
    core::mem::forget(ppi_ch);
    core::mem::forget(clkin_ch);

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
        imu_config.gyro.odr = GyroOdr::_200Hz;
        imu_config.accel.odr = AccelOdr::_200Hz;
        imu_config.accel.mode = AccelMode::LowNoise;
        imu_config.int1.drive = Drive::PushPull;
        imu_config.int1.polarity = Polarity::ActiveLow;
        imu_config.pin9.function = Pin9Function::CLKIN;
        imu_config.fifo_watermark = 1;
    }
    let imu = imu.initialize(Delay, imu_config).await.unwrap();
    (imu, ImuInterrupt { int1 })
}

pub struct ImuInterrupt {
    int1: Input<'static>,
}

impl ImuInterrupt {
    pub fn wait(&mut self) -> impl Future<Output = ()> {
        self.int1.wait_for_low()
    }
}
