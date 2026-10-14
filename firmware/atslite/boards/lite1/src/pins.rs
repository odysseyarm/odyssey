//! Pin definitions for lite1 board (POC2 schematic)
//!
//! Based on Radiosity_POC2.PDF schematic (ATS POC2 Lite1, Rev A)

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

// Vision sensor (PAG7661QN via SPIM4 high-speed bus)
// Sheet 3: SPI4_CS=P1.13, SPI4_SCK=P1.08, SPI4_MOSI=P0.29, SPI4_MISO=P1.09
// Sheet 3: VS_INT_O=P1.14, VS_RESET_N=P0.24, VS_LED_HIRL=P1.10
define_peripheral!(Pag7665qn {
    sck: P0_08,
    miso: P0_10,
    mosi: P0_09,
    cs: P0_11,
    int_o: P1_15,
    reset: P0_24,
    led_ctrl: P1_10,
});
impl_spi_from!(Pag7665qn {
    sck: sck,
    miso: miso,
    mosi: mosi,
    cs: cs
});

// Primary IMU (ICM-42688-P via SPI0)
// Sheet 3: SPI0_CS=P0.22, SPI0_SCK=P0.20, SPI0_MOSI=P0.19, SPI0_MISO=P1.00
// Sheet 3: ICM_INT1=P0.21, ICM_CLKIN=P1.05
define_peripheral!(Icm42688p {
    sck: P1_08,
    miso: P1_09,
    mosi: P0_29,
    cs: P1_13,
    int1: P0_21,
    clkin: P1_05,
});
impl_spi_from!(Icm42688p {
    sck: sck,
    miso: miso,
    mosi: mosi,
    cs: cs
});

// PMIC (NPM1300 via I2C)
// Sheet 3: I2C_SCL3=P1.03, I2C_SDA3=P1.02
// Sheet 3: PM_INT=P0.05, PM_PFAIL=P0.06
// Sheet 5: PM_SHPHLD
// Sheet 6: PWR_BTN=P0.31
define_peripheral!(Pmic {
    sda: P1_02,
    scl: P1_03,
    shphld: P0_30,
    int: P0_05,
    fail: P0_06,
    pwr_btn: P0_04,
});

group_peripherals!(Board {
    pag7665qn: Pag7665qn { sck=P0_08, miso=P0_10, mosi=P0_09, cs=P0_11, int_o=P1_15, reset=P0_24, led_ctrl=P1_10 },
    icm42688p: Icm42688p { sck=P1_08, miso=P1_09, mosi=P0_29, cs=P1_13, int1=P0_21, clkin=P1_05 },
    pmic:      Pmic      { sda=P1_02, scl=P1_03, shphld=P0_30, int=P0_05, fail=P0_06, pwr_btn=P0_04 },
});
