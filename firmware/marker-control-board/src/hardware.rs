// Ported from https://github.com/odysseyarm/marker-control-board
// revision 9c0779478329d84e9bfaafd6b11119c7a6ee3dea (main.rs and pinout.rs).
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::twim::{self, Twim};
use embassy_nrf::{pac, peripherals};
use embassy_time::Duration;

pub const DEVICE_INTERFACE_GUIDS: &[&str] = &["{265744E6-40E8-41C4-AE17-F6F2EB187F1C}"];

embassy_nrf::bind_interrupts!(pub struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler;
    TWISPI0 => twim::InterruptHandler<peripherals::TWISPI0>;
});

/// Also used before HAL initialization and in fatal/reset paths; never raises an output.
pub fn force_markers_off() {
    pac::P0.outclr().write(|w| {
        w.set_pin(15, true);
        w.set_pin(17, true);
        w.set_pin(20, true);
    });
    pac::P1.outclr().write(|w| w.set_pin(9, true));
    pac::P0.dirset().write(|w| {
        w.set_pin(15, true);
        w.set_pin(17, true);
        w.set_pin(20, true);
    });
    pac::P1.dirset().write(|w| w.set_pin(9, true));
}

pub fn reset_safe() -> ! {
    cortex_m::interrupt::disable();
    force_markers_off();
    cortex_m::peripheral::SCB::sys_reset()
}

pub fn usb_powered() -> bool {
    pac::POWER.usbregstatus().read().vbusdetect()
}

pub fn clear_reset_reason() {
    let reason = pac::POWER.resetreas().read();
    pac::POWER.resetreas().write_value(reason);
}

pub fn serial_number() -> [u8; 12] {
    let low = pac::FICR.deviceid(0).read().to_le_bytes();
    let high = pac::FICR.deviceid(1).read().to_le_bytes();
    let bytes = [low[0], low[1], low[2], low[3], high[0], high[1]];
    let mut serial = [0; 12];
    for (i, byte) in bytes.into_iter().enumerate() {
        serial[i * 2] = b"0123456789ABCDEF"[(byte >> 4) as usize];
        serial[i * 2 + 1] = b"0123456789ABCDEF"[(byte & 15) as usize];
    }
    serial
}

pub struct Markers<'d> {
    outputs: [Output<'d>; 4],
    twi: Twim<'d>,
}

impl<'d> Markers<'d> {
    pub fn new(
        p: embassy_nrf::Peripherals,
        tx_ram_buffer: &'d mut [u8],
    ) -> (
        Self,
        Input<'static>,
        embassy_nrf::Peri<'static, peripherals::USBD>,
        embassy_nrf::Peri<'static, peripherals::NVMC>,
    ) {
        let outputs = [
            Output::new(p.P0_15, Level::Low, OutputDrive::Standard),
            Output::new(p.P1_09, Level::Low, OutputDrive::Standard),
            Output::new(p.P0_17, Level::Low, OutputDrive::Standard),
            Output::new(p.P0_20, Level::Low, OutputDrive::Standard),
        ];
        let twi = Twim::new(
            p.TWISPI0,
            Irqs,
            p.P0_10,
            p.P0_09,
            twim::Config::default(),
            tx_ram_buffer,
        );
        (
            Self { outputs, twi },
            Input::new(p.P0_11, Pull::Up),
            p.USBD,
            p.NVMC,
        )
    }
    /// PCAL6408A ADDR=false (0x20): io0 ISET1=1, io1 ISET0=0, io2 DRIVEV=0.
    pub fn configure_off(&mut self) -> Result<(), twim::Error> {
        self.set(false)?;
        let mut direction = [0];
        self.twi.blocking_write_read_timeout(
            0x20,
            &[0x03],
            &mut direction,
            Duration::from_millis(100),
        )?;
        self.twi.blocking_write_timeout(
            0x20,
            &[0x03, direction[0] & !0x07],
            Duration::from_millis(100),
        )
    }

    pub fn set(&mut self, on: bool) -> Result<(), twim::Error> {
        for output in &mut self.outputs {
            output.set_low();
        }
        let mut value = [0];
        self.twi.blocking_write_read_timeout(
            0x20,
            &[0x01],
            &mut value,
            Duration::from_millis(100),
        )?;
        // Preserve unrelated expander pins and upstream current-selection bits.
        let value = (value[0] & !0x07) | 0x01 | if on { 0x04 } else { 0 };
        self.twi
            .blocking_write_timeout(0x20, &[0x01, value], Duration::from_millis(100))?;
        if on {
            for output in &mut self.outputs {
                output.set_high();
            }
        }
        Ok(())
    }
}
