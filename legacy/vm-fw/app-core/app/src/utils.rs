use embassy_nrf::gpio::{self, Level, Output, OutputDrive};
use embassy_nrf::interrupt::typelevel::Binding;
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::{Peri, pac};
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
    #[cfg(context = "vm")]
    let low = ficr.deviceid(0).read();
    #[cfg(context = "vm")]
    let high = ficr.deviceid(1).read();
    #[cfg(context = "atslite1")]
    let low = ficr.info().deviceid(0).read();
    #[cfg(context = "atslite1")]
    let high = ficr.info().deviceid(1).read();
    let [a, b, c, d] = low.to_le_bytes();
    let [e, f, g, h] = high.to_le_bytes();
    [a, b, c, d, e, f, g, h]
}

#[cfg(context = "atslite1")]
#[macro_export]
macro_rules! battery_model_inc {
    (
        .param_1  = { $($p1:expr),* $(,)? },
        .temps    = { $($temps:expr),* $(,)? },
        .param_2  = { $($p2:expr),* $(,)? },
        .param_3  = { $($p3:expr),* $(,)? },
        .param_4  = { $($p4:expr),* $(,)? },
        .param_5  = { $($p5:expr),* $(,)? },
        .param_6  = { $($p6:expr),* $(,)? },
        .param_7  = { $($p7:expr),* $(,)? },
        .param_8  = { $($p8:expr),* $(,)? },
        .param_9  = { $($p9:expr),* $(,)? },
        .param_10 = { $($p10:expr),* $(,)? },
        .param_11 = { $($p11:expr),* $(,)? },
        .param_12 = { $($p12:expr),* $(,)? },
        name: $name:literal,
    ) => {
        mod __battery_model_generated {
            use nrfxlib_sys as sys;

            // Fixed sizes from the C struct
            pub static PARAM_1:  [f32; 201] = [ $($p1),* ];
            pub static TEMPS:    [f32;   3] = [ $($temps),* ];
            pub static PARAM_2:  [f32; 122] = [ $($p2),* ];
            pub static PARAM_3:  [f32; 201] = [ $($p3),* ];
            pub static PARAM_4:  [f32; 201] = [ $($p4),* ];
            pub static PARAM_5:  [f32; 201] = [ $($p5),* ];
            pub static PARAM_6:  [f32; 201] = [ $($p6),* ];
            pub static PARAM_7:  [f32; 122] = [ $($p7),* ];
            pub static PARAM_8:  [f32; 122] = [ $($p8),* ];
            pub static PARAM_9:  [f32;   3] = [ $($p9),* ];
            pub static PARAM_10: [f32;   6] = [ $($p10),* ];
            pub static PARAM_11: [f32;   6] = [ $($p11),* ];
            pub static PARAM_12: [f32;   3] = [ $($p12),* ];

            const fn pad_name(s: &str) -> [u8; 64] {
                let bs = s.as_bytes();
                let mut out = [0u8; 64];
                let mut i = 0;
                while i < bs.len() && i < 64 {
                    out[i] = bs[i];
                    i += 1;
                }
                out
            }
            pub static NAME: [u8; 64] = pad_name($name);

            pub static BATTERY_MODEL: sys::battery_model = sys::battery_model {
                param_1: PARAM_1,
                temps:   TEMPS,
                param_2: PARAM_2,
                param_3: PARAM_3,
                param_4: PARAM_4,
                param_5: PARAM_5,
                param_6: PARAM_6,
                param_7: PARAM_7,
                param_8: PARAM_8,
                param_9:  PARAM_9,
                param_10: PARAM_10,
                param_11: PARAM_11,
                param_12: PARAM_12,
                name: NAME,
            };
        }
        pub use __battery_model_generated::BATTERY_MODEL;
    };
}

pub fn set_pin_sense(port: &gpio::Port, idx: u8, sense: pac::gpio::vals::Sense) {
    let n = idx as usize;
    match port {
        gpio::Port::Port0 => pac::P0.pin_cnf(n).modify(|w| {
            w.set_dir(nrf_pac::gpio::vals::Dir::INPUT);
            w.set_input(nrf_pac::gpio::vals::Input::CONNECT);
            w.set_pull(nrf_pac::gpio::vals::Pull::PULLUP);
            w.set_drive(nrf_pac::gpio::vals::Drive::S0S1);
            w.set_sense(sense)
        }),
        gpio::Port::Port1 => pac::P1.pin_cnf(n).modify(|w| w.set_sense(sense)),
    }
}
