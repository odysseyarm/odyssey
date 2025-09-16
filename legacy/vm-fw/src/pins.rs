use embassy_nrf::gpio::AnyPin;
use embassy_nrf::peripherals::*;
use embassy_nrf::Peripherals;

pub struct Spi {
    pub sck: AnyPin,
    pub miso: AnyPin,
    pub mosi: AnyPin,
    pub cs: AnyPin,
}

macro_rules! define_peripheral {
    ($name:ident { $( $field:ident : $pin:ident ),+ $(,)? }) => {
        pub struct $name { $( pub $field: $pin ),+ }
    };
}

macro_rules! impl_spi_from {
    ($name:ident { sck:$sck:ident, miso:$miso:ident, mosi:$mosi:ident, cs:$cs:ident }) => {
        impl From<$name> for Spi {
            fn from(p: $name) -> Spi {
                Spi { sck: p.$sck.into(), miso: p.$miso.into(), mosi: p.$mosi.into(), cs: p.$cs.into() }
            }
        }
    };
}

macro_rules! group_peripherals {
    ($board:ident {
        $(
            $field:ident : $bag:ident { $( $bfield:ident = $pin:ident ),+ $(,)? }
        ),+ $(,)?
    }) => {
        pub struct $board { $( pub $field: $bag ),+ }

        impl $board {
            pub fn from_peripherals(p: Peripherals) -> Self {
                Self {
                    $(
                        $field: $bag { $( $bfield: *p.$pin ),+ }
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
    cs:  P0_08,
});
#[cfg(context = "vm")]
impl_spi_from!(Paj7025R2 { sck:sck, miso:miso, mosi:mosi, cs:cs });

#[cfg(context = "vm")]
define_peripheral!(Paj7025R3 {
    sck: P1_00,
    miso: P0_22,
    mosi: P1_07,
    cs:  P0_06,
});
#[cfg(context = "vm")]
impl_spi_from!(Paj7025R3 { sck:sck, miso:miso, mosi:mosi, cs:cs });

#[cfg(context = "vm")]
define_peripheral!(Bmi270 {
    sck: P0_20,
    miso: P0_19,
    mosi: P0_15,
    cs:  P0_03,
    irq: P0_25,
});
#[cfg(context = "vm")]
impl_spi_from!(Bmi270 { sck:sck, miso:miso, mosi:mosi, cs:cs });

#[cfg(context = "vm")]
group_peripherals!(Board {
    paj7025r2_spi: Paj7025R2 { sck=P0_05, miso=P0_04, mosi=P0_26, cs=P0_08 },
    paj7025r3_spi: Paj7025R3 { sck=P1_00, miso=P0_22, mosi=P1_07, cs=P0_06 },
    bmi270_spi:    Bmi270    { sck=P0_20, miso=P0_19, mosi=P0_15, cs=P0_03, irq=P0_25 },
});
