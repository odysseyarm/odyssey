use bytemuck::{cast_slice, from_bytes};
use embassy_nrf::{
    Peripheral,
    gpio::{AnyPin, Input, Level, Output, OutputDrive, Pull},
    gpiote::{self, OutputChannelPolarity},
    interrupt,
    ppi::{self, Ppi},
    spim,
    timer::{self, Timer},
};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use icm426xx::{fifo::FifoPacket4, ICM42688};
use static_cell::ConstStaticCell;

pub async fn init<'d, T>(
    spim_instance: impl Peripheral<P = T> + 'd,
    cs: AnyPin,
    sck: AnyPin,
    miso: AnyPin,
    mosi: AnyPin,
    int1: AnyPin,
    clkin: AnyPin,
    clkin_timer_instance: impl Peripheral<P: timer::Instance>,
    clkin_ppi_ch: ppi::AnyConfigurableChannel,
    clkin_gpiote_ch: gpiote::AnyChannel,
) where
    T: spim::Instance,
    crate::Irqs: interrupt::typelevel::Binding<T::Interrupt, spim::InterruptHandler<T>> + 'd,
{
    let mut int1 = Input::new(int1, Pull::None);
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

    let mut spim_config = spim::Config::default();
    spim_config.frequency = spim::Frequency::M8;
    spim_config.mode = spim::MODE_3;
    spim_config.bit_order = spim::BitOrder::MSB_FIRST;
    let spim = spim::Spim::new(spim_instance, crate::Irqs, sck, miso, mosi, spim_config);

    let Ok(spi_device) = ExclusiveDevice::new(spim, cs, Delay);
    let imu = ICM42688::new(spi_device);
    let mut imu_config = icm426xx::Config::default();
    {
        use icm426xx::config::{AccelOdr, AccelMode, Drive, GyroOdr, Pin9Function, Polarity};
        imu_config.gyro.odr = GyroOdr::_200Hz;
        imu_config.accel.odr = AccelOdr::_200Hz;
        imu_config.accel.mode = AccelMode::LowNoise;
        imu_config.int1.drive = Drive::PushPull;
        imu_config.int1.polarity = Polarity::ActiveLow;
        imu_config.pin9.function = Pin9Function::CLKIN;
        imu_config.fifo_watermark = 1;
    }
    let mut imu = imu.initialize(Delay, imu_config).await.unwrap();
    static BUFFER: ConstStaticCell<[u32; 521]> = ConstStaticCell::new([0; 521]);
    let buffer = BUFFER.take();
    loop {
        int1.wait_for_low().await;
        let items = imu.read_fifo(buffer).await.unwrap();
        let pkt = from_bytes::<FifoPacket4>(&cast_slice(buffer)[4..24]);
        let ts = pkt.timestamp();
        let ax = pkt.accel_data_x();
        let ay = pkt.accel_data_y();
        let az = pkt.accel_data_z();
        let gx = pkt.gyro_data_x();
        let gy = pkt.gyro_data_y();
        let gz = pkt.gyro_data_z();
        defmt::info!("IMU {=usize} fifo items, {}", items, pkt.fifo_header());
        defmt::info!("ts={} ax={} ay={} az={} gx={} gy={} gz={}", ts, ax, ay, az, gx, gy, gz);
    }
}
