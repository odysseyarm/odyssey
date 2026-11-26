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
    sck: P1_08,
    miso: P1_09,
    mosi: P0_29,
    cs: P1_13,
    fod: P1_14,
});

define_peripheral!(Paj7025R3 {
    sck: P0_20,
    miso: P0_22,
    mosi: P1_01,
    cs: P1_04,
    fod: P0_24,
});

define_peripheral!(Icm42688v {
    sck: P0_09,
    miso: P0_12,
    mosi: P0_10,
    cs: P0_08,
    int1: P0_21,
    clkin: P1_05,
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
