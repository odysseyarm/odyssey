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

define_peripheral!(Paj7025R2 {
    sck: P1_08,
    miso: P1_09,
    mosi: P0_29,
    cs: P1_13,
    fod: P1_14,
});
impl_spi_from!(Paj7025R2 {
    sck: sck,
    miso: miso,
    mosi: mosi,
    cs: cs
});

define_peripheral!(Paj7025R3 {
    sck: P0_20,
    miso: P0_22,
    mosi: P1_01,
    cs: P1_04,
    fod: P0_24,
});
impl_spi_from!(Paj7025R3 {
    sck: sck,
    miso: miso,
    mosi: mosi,
    cs: cs
});

define_peripheral!(Icm42688v {
    sck: P0_09,
    miso: P0_12,
    mosi: P0_10,
    cs: P0_08,
    int1: P0_21,
    clkin: P1_05,
});
impl_spi_from!(Icm42688v {
    sck: sck,
    miso: miso,
    mosi: mosi,
    cs: cs
});

define_peripheral!(Pmic {
    sda: P1_02,
    scl: P1_03,
    pwr_btn: P0_04,
    irq: P0_05
});

group_peripherals!(Board {
    paj7025r2: Paj7025R2 { sck=P1_08, miso=P1_09, mosi=P0_29, cs=P1_13, fod=P1_14 },
    paj7025r3: Paj7025R3 { sck=P0_20, miso=P0_22, mosi=P1_01, cs=P1_04, fod=P0_24 },
    icm42688v: Icm42688v { sck=P0_09, miso=P0_12, mosi=P0_10, cs=P0_08, int1=P0_21, clkin=P1_05 },
    pmic:      Pmic     { sda=P1_02, scl=P1_03, pwr_btn=P0_04, irq=P0_05 },
});
