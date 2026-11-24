use embassy_nrf::Peri;
use embassy_nrf::gpio::{Level, Output, OutputDrive, Pin};
use embassy_nrf::interrupt::typelevel::Binding;
use embassy_nrf::mode::Async;
use embassy_nrf::spim::{self, Spim};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;

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
        static BUFFER: static_cell::ConstStaticCell<[u8; $size]> =
            static_cell::ConstStaticCell::new([0; $size]);
        BUFFER.take()
    }};
}

pub(crate) use static_byte_buffer;

/// Since: Odyssey Desktop 0.7.0.
#[expect(
    clippy::too_many_arguments,
    reason = "Each peripheral and pin is a distinct typed resource"
)]
pub fn make_spi_dev<'d, SPI, IRQ>(
    spi: Peri<'d, SPI>,
    irqs: IRQ,
    sck: Peri<'d, impl spim::SckPin<SPI>>,
    mosi: Peri<'d, impl spim::MosiPin<SPI>>,
    miso: Peri<'d, impl spim::MisoPin<SPI>>,
    cs: Peri<'d, impl Pin>,
    cfg: spim::Config,
) -> ExclusiveDevice<Spim<'d, Async>, Output<'d>, Delay>
where
    SPI: spim::Instance,
    IRQ: Binding<<SPI as spim::Instance>::Interrupt, spim::InterruptHandler<SPI>> + 'd,
{
    let spim = Spim::new(spi, sck, mosi, miso, irqs, cfg);
    let cs_out = Output::new(cs, Level::High, OutputDrive::Standard);
    ExclusiveDevice::new(spim, cs_out, Delay).unwrap()
}
