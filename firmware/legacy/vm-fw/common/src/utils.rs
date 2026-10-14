use embassy_nrf::Peri;
use embassy_nrf::gpio::{AnyPin, Level, Output, OutputDrive};
use embassy_nrf::interrupt::typelevel::Binding;
use embassy_nrf::spim::{self, Spim};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;

// Generic SPI pin bundle
pub struct SpiPins {
    pub sck: Peri<'static, AnyPin>,
    pub miso: Peri<'static, AnyPin>,
    pub mosi: Peri<'static, AnyPin>,
    pub cs: Peri<'static, AnyPin>,
}

macro_rules! impl_from {
    ($enum:ident :: $variant:ident ( $ty:ty )) => {
        impl From<$ty> for $enum {
            fn from(t: $ty) -> Self {
                Self::$variant(t)
            }
        }
    };
}

pub(crate) use impl_from;

macro_rules! static_byte_buffer {
    ($size:expr) => {{
        static BUFFER: static_cell::ConstStaticCell<[u8; $size]> = static_cell::ConstStaticCell::new([0; $size]);
        BUFFER.take()
    }};
}

pub(crate) use static_byte_buffer;

pub fn make_spi_dev<SPI, IRQ>(
    spi: Peri<'static, SPI>,
    irqs: IRQ,
    pins: SpiPins,
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
