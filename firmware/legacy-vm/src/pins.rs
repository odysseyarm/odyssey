use embassy_nrf::Peri;
use embassy_nrf::peripherals::*;

macro_rules! define_peripheral {
    ($name:ident { $( $field:ident : $pin:ident ),+ $(,)? }) => {
        pub struct $name { $( pub $field: Peri<'static, $pin> ),+ }
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
    sck: P0_05,
    miso: P0_04,
    mosi: P0_26,
    cs: P0_08,
    fod: P0_12,
});

define_peripheral!(Paj7025R3 {
    sck: P1_00,
    miso: P0_22,
    mosi: P1_07,
    cs: P0_06,
    fod: P1_13,
});

define_peripheral!(Bmi270 {
    sck: P0_20,
    miso: P0_19,
    mosi: P0_15,
    cs: P0_03,
    irq: P0_25,
});

group_peripherals!(Board {
    paj7025r2: Paj7025R2 { sck=P0_05, miso=P0_04, mosi=P0_26, cs=P0_08, fod=P0_12 },
    paj7025r3: Paj7025R3 { sck=P1_00, miso=P0_22, mosi=P1_07, cs=P0_06, fod=P1_13 },
    bmi270:    Bmi270    { sck=P0_20, miso=P0_19, mosi=P0_15, cs=P0_03, irq=P0_25 },
});
