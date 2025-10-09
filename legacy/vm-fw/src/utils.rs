use embassy_nrf::Peri;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::interrupt::typelevel::Binding;
use embassy_nrf::spim::{self, Spim};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;

use crate::pins;

macro_rules! impl_from {
    ($enum:ident :: $variant:ident ( $ty:ty )) => {
        impl From<$ty> for $enum {
            fn from(t: $ty) -> Self {
                Self::$variant(t)
            }
        }
    };
}

macro_rules! static_byte_buffer {
    ($size:expr) => {{
        static BUFFER: ConstStaticCell<[u8; $size]> = ConstStaticCell::new([0; $size]);
        BUFFER.take()
    }};
}

pub fn make_spi_dev<SPI, IRQ>(
    spi: Peri<'static, SPI>,
    irqs: IRQ,
    pins: pins::Spi,
    cfg: spim::Config,
) -> ExclusiveDevice<Spim<'static>, Output<'static>, Delay>
where
    SPI: spim::Instance,
    IRQ: Binding<<SPI as spim::Instance>::Interrupt, spim::InterruptHandler<SPI>> + 'static,
{
    let spim = Spim::new(spi, irqs, pins.sck, pins.miso, pins.mosi, cfg);
    let cs_out = Output::new(pins.cs, Level::High, OutputDrive::Standard);
    ExclusiveDevice::new(spim, cs_out, Delay).unwrap()
}

pub fn device_id() -> [u8; 8] {
    let ficr = embassy_nrf::pac::FICR;
    let low = ficr.deviceid(0).read();
    let high = ficr.deviceid(1).read();
    let [a, b, c, d] = low.to_le_bytes();
    let [e, f, g, h] = high.to_le_bytes();
    [a, b, c, d, e, f, g, h]
}
