// Ported from https://github.com/odysseyarm/marker-control-board
// revision 9c0779478329d84e9bfaafd6b11119c7a6ee3dea (main.rs and pinout.rs).
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::twim::{self, Twim};
use embassy_nrf::{pac, peripherals};
use embassy_time::Duration;

const LED_MASK: u8 = 0xE0;

pub const DEVICE_INTERFACE_GUIDS: &[&str] = &["{265744E6-40E8-41C4-AE17-F6F2EB187F1C}"];

embassy_nrf::bind_interrupts!(pub struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
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

pub struct MarkerPins {
    pub output_1: embassy_nrf::Peri<'static, peripherals::P0_15>,
    pub output_2: embassy_nrf::Peri<'static, peripherals::P1_09>,
    pub output_3: embassy_nrf::Peri<'static, peripherals::P0_17>,
    pub output_4: embassy_nrf::Peri<'static, peripherals::P0_20>,
    pub twi: embassy_nrf::Peri<'static, peripherals::TWISPI0>,
    pub scl: embassy_nrf::Peri<'static, peripherals::P0_10>,
    pub sda: embassy_nrf::Peri<'static, peripherals::P0_09>,
}

pub struct Markers<'d> {
    outputs: [Output<'d>; 4],
    twi: Twim<'d>,
}

impl<'d> Markers<'d> {
    pub fn new(p: MarkerPins, tx_ram_buffer: &'d mut [u8]) -> Self {
        let outputs = [
            Output::new(p.output_1, Level::Low, OutputDrive::Standard),
            Output::new(p.output_2, Level::Low, OutputDrive::Standard),
            Output::new(p.output_3, Level::Low, OutputDrive::Standard),
            Output::new(p.output_4, Level::Low, OutputDrive::Standard),
        ];
        let twi = Twim::new(
            p.twi,
            Irqs,
            p.scl,
            p.sda,
            twim::Config::default(),
            tx_ram_buffer,
        );
        Self { outputs, twi }
    }
    /// PCAL6408A ADDR=false (0x20): io0 ISET1=1, io1 ISET0=0, io2 DRIVEV=0.
    pub fn configure_off(&mut self) -> Result<(), twim::Error> {
        // Output register powers up as 0xFF: clear the LED bits before they become outputs.
        self.set_led(0)?;
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
            &[0x03, direction[0] & !(0x07 | LED_MASK)],
            Duration::from_millis(100),
        )
    }

    /// `bits` are the PCAL6408A P5..P7 positions of the RGB LED; other pins keep their value.
    pub fn set_led(&mut self, bits: u8) -> Result<(), twim::Error> {
        let mut value = [0];
        self.twi.blocking_write_read_timeout(
            0x20,
            &[0x01],
            &mut value,
            Duration::from_millis(100),
        )?;
        let value = (value[0] & !LED_MASK) | bits;
        self.twi
            .blocking_write_timeout(0x20, &[0x01, value], Duration::from_millis(100))
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
