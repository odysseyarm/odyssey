use ariel_nrf::gpio::AnyPin;
use ariel_os::hal::peripherals;

/// One unified, type-erased pin bag usable by both R2 and R3.
pub struct Spi {
    pub sck: AnyPin,
    pub miso: AnyPin,
    pub mosi: AnyPin,
    pub cs: AnyPin,
}

#[cfg(context = "nrf52840")]
ariel_os::hal::define_peripherals!(Paj7025R2 {
    sck: P0_05,
    miso: P0_04,
    mosi: P0_26,
    cs: P0_08,
});

#[cfg(context = "nrf52840")]
ariel_os::hal::define_peripherals!(Paj7025R3 {
    sck: P1_00,
    miso: P0_22,
    mosi: P1_07,
    cs: P0_06,
});

#[cfg(context = "nrf52840")]
ariel_os::hal::define_peripherals!(Bmi270 {
    sck: P0_20,
    miso: P0_19,
    mosi: P0_15,
    cs: P0_03,
    irq: P0_25,
});

impl From<Paj7025R2> for Spi {
    fn from(p: Paj7025R2) -> Self {
        Self {
            sck: p.sck.into(),
            miso: p.miso.into(),
            mosi: p.mosi.into(),
            cs: p.cs.into(),
        }
    }
}

impl From<Paj7025R3> for Spi {
    fn from(p: Paj7025R3) -> Self {
        Self {
            sck: p.sck.into(),
            miso: p.miso.into(),
            mosi: p.mosi.into(),
            cs: p.cs.into(),
        }
    }
}

impl From<Bmi270> for Spi {
    fn from(p: Bmi270) -> Self {
        Self {
            sck: p.sck.into(),
            miso: p.miso.into(),
            mosi: p.mosi.into(),
            cs: p.cs.into(),
        }
    }
}

#[cfg(context = "nrf52840")]
ariel_os::hal::group_peripherals!(Peripherals {
    paj7025r2_spi: Paj7025R2,
    paj7025r3_spi: Paj7025R3,
    bmi270_spi: Bmi270,
});
