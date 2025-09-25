use embassy_nrf::Peri;
use embassy_nrf::gpio::AnyPin;
use embassy_nrf::peripherals::*;

pub struct Spi {
    pub sck: Peri<'static, AnyPin>,
    pub miso: Peri<'static, AnyPin>,
    pub mosi: Peri<'static, AnyPin>,
    pub cs: Peri<'static, AnyPin>,
}

macro_rules! define_peripheral {
    ($name:ident { $( $field:ident : $pin:ident ),+ $(,)? }) => {
        pub struct $name { $( pub $field: Peri<'static, $pin> ),+ }
    };
}

macro_rules! impl_spi_from {
    ($name:ident { sck:$sck:ident, miso:$miso:ident, mosi:$mosi:ident, cs:$cs:ident }) => {
        impl From<$name> for Spi {
            fn from(p: $name) -> Spi {
                Spi {
                    sck: p.$sck.into(),
                    miso: p.$miso.into(),
                    mosi: p.$mosi.into(),
                    cs: p.$cs.into(),
                }
            }
        }
    };
}

macro_rules! group_peripherals {
    ($board:ident {
        $( $field:ident : $bag:ident { $( $bfield:ident = $pin:ident ),+ $(,)? } ),+ $(,)?
    }) => {
        pub struct $board { $( pub $field: $bag ),+ }

        #[macro_export]
        macro_rules! split_board {
            ($p:ident) => {
                $crate::pins::$board {
                    $(
                        $field: $crate::pins::$bag { $( $bfield: $p.$pin ),+ }
                    ),+
                }
            }
        }
    };
}

#[cfg(context = "vm")]
define_peripheral!(Paj7025R2 {
    sck: P0_05,
    miso: P0_04,
    mosi: P0_26,
    cs: P0_08,
    fod: P0_12,
});
#[cfg(context = "vm")]
impl_spi_from!(Paj7025R2 {
    sck: sck,
    miso: miso,
    mosi: mosi,
    cs: cs
});

#[cfg(context = "vm")]
define_peripheral!(Paj7025R3 {
    sck: P1_00,
    miso: P0_22,
    mosi: P1_07,
    cs: P0_06,
    fod: P1_13,
});
#[cfg(context = "vm")]
impl_spi_from!(Paj7025R3 {
    sck: sck,
    miso: miso,
    mosi: mosi,
    cs: cs
});

#[cfg(context = "vm")]
define_peripheral!(Bmi270 {
    sck: P0_20,
    miso: P0_19,
    mosi: P0_15,
    cs: P0_03,
    irq: P0_25,
});
#[cfg(context = "vm")]
impl_spi_from!(Bmi270 {
    sck: sck,
    miso: miso,
    mosi: mosi,
    cs: cs
});

#[cfg(context = "vm")]
group_peripherals!(Board {
    paj7025r2: Paj7025R2 { sck=P0_05, miso=P0_04, mosi=P0_26, cs=P0_08, fod=P0_12 },
    paj7025r3: Paj7025R3 { sck=P1_00, miso=P0_22, mosi=P1_07, cs=P0_06, fod=P1_13 },
    bmi270:    Bmi270    { sck=P0_20, miso=P0_19, mosi=P0_15, cs=P0_03, irq=P0_25 },
});
