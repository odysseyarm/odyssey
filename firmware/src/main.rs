#![no_std]
#![no_main]

#[macro_use]
mod pinout;

use defmt::info;
use embassy_executor::Spawner;
use embassy_futures::join::{join, join3};
use embassy_nrf::{
    config::{Config, HfclkSource}, peripherals, spim::{self, Spim}, spis::{self, Spis}, usb::{
        self,
        vbus_detect::HardwareVbusDetect,
        Driver,
    }, Peripheral
};
use embassy_sync::{blocking_mutex::raw::{NoopRawMutex, RawMutex}, channel::{Channel, Receiver, Sender}};
use embassy_usb::{
    class::cdc_acm::{CdcAcmClass, State}, driver::EndpointError, Builder, UsbDevice
};
use paj7025_nrf::Paj7025;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

#[link_section = ".ramtextload"]
#[cortex_m_rt::pre_init]
unsafe fn init_ramtext() {
    core::arch::asm!(
        // Initialise .data memory. `__sdata`, `__sidata`, and `__edata` come from the linker script.
        "ldr r0, =__sramtext",
        "ldr r1, =__eramtext",
        "ldr r2, =__siramtext",
        "2:",
        "cmp r1, r0",
        "beq 3f",
        "ldm r2!, {{r3}}",
        "stm r0!, {{r3}}",
        "b 2b",
        "3:",
    );

    #[cfg(feature = "mcuboot")]
    core::arch::asm!(
        "cpsie i",  // Clear primask

        // Clear SHPR3
        "ldr r0, =0xE000ED20",
        "ldr r1, =0x0",
        "str r1, [r0]",
        "str r1, [r0, #4]", // Clear SHCSR

        // Clear SCR
        "ldr r0, =0xE000ED10",
        "str r1, [r0]",
        "str r1, [r0, #4]", // Clear CCR
    );
}

// this could go in pre_init, but it already works so /shrug
#[cfg(feature = "vm2")]
fn check_regout0() {
    use embassy_nrf::pac::{self, power::mainregstatus::MAINREGSTATUS_A};
    // if the ats_mot_nrf52840 board is powered from USB
    // (high voltage mode), GPIO output voltage is set to 1.8 volts by
    // default and that is not enough for the vision sensors.
    // Increase GPIO voltage to 2.4 volts.

    let mainregstatus = unsafe { &*pac::POWER::ptr() }
        .mainregstatus
        .read()
        .mainregstatus()
        .variant();
    match mainregstatus {
        MAINREGSTATUS_A::NORMAL => return,
        MAINREGSTATUS_A::HIGH => (),
    }

    let regout0 = &unsafe { &*pac::UICR::ptr() }.regout0;
    if !regout0.read().vout().is_default() {
        return;
    }

    // Enable writing to flash
    let nvmc = unsafe { &*pac::NVMC::ptr() };
    nvmc.config.write(|w| w.wen().wen());
    while !nvmc.ready.read().ready().is_ready() {}

    // Set REGOUT0 to 2.4 volts
    regout0.write(|w| w.vout()._2v4());

    // Wait for write to finish
    while !nvmc.ready.read().ready().is_ready() {}

    pac::SCB::sys_reset()
}

#[cfg(any(feature = "atslite-1-1", feature = "atslite-2-2", feature = "atslite-4-1"))]
embassy_nrf::bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => usb::vbus_detect::InterruptHandler;
    SERIAL0 => spim::InterruptHandler<peripherals::SERIAL0>;
    SERIAL1 => spim::InterruptHandler<peripherals::SERIAL1>;
    SERIAL2 => spis::InterruptHandler<peripherals::SERIAL2>;
    SERIAL3 => spis::InterruptHandler<peripherals::SERIAL3>;
});

#[cfg(feature = "vm2")]
embassy_nrf::bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    POWER_CLOCK => usb::vbus_detect::InterruptHandler;
    SPIM3 => spim::InterruptHandler<peripherals::SPI3>;
    SPIM1_SPIS1_TWIM1_TWIS1_SPI1_TWI1 => spim::InterruptHandler<peripherals::TWISPI1>;
    SPIM2_SPIS2_SPI2 => spis::InterruptHandler<peripherals::SPI2>;
    SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0 => spis::InterruptHandler<peripherals::TWISPI0>;
});

// fn log_stuff() {
//     let pc = cortex_m::register::pc::read();
//     info!("Hello from {:010x}", pc);
//     let mainregstatus = unsafe { &*pac::POWER::ptr() }
//         .mainregstatus
//         .read()
//         .mainregstatus()
//         .variant();
//     info!("MAINREGSTATUS is {}", Debug2Format(&mainregstatus));
//     let regout0 = unsafe { &*pac::UICR::ptr() }
//         .regout0
//         .read()
//         .vout()
//         .variant();
//     info!("REGOUT0 is {}", Debug2Format(&regout0));
//     let hfclkstat = unsafe { &*pac::CLOCK::ptr() }.hfclkstat.read();
//     info!(
//         "HFCLKSTAT_SRC is {}",
//         Debug2Format(&hfclkstat.src().variant())
//     );
//     info!(
//         "HFCLKSTAT_STATE is {}",
//         Debug2Format(&hfclkstat.state().variant())
//     );
// }

const NUM_BUFFERS: usize = 2;
static SHARED_BUFFERS: StaticCell<[[u8; 98*98 + 98*3]; NUM_BUFFERS]> = StaticCell::new();

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    #[cfg(feature = "vm2")]
    check_regout0();

    let shared_buffers = SHARED_BUFFERS.init_with(|| [[0; 98*98+98*3]; NUM_BUFFERS]);
    let mut config = Config::default();
    config.hfclk_source = HfclkSource::ExternalXtal;
    let mut p = embassy_nrf::init(config);

    // log_stuff();

    let mut wide = Paj7025::new(
        Spim::new(
            &mut pinout!(p.wf_spim),
            Irqs,
            &mut pinout!(p.wf_sck),
            &mut pinout!(p.wf_miso),
            &mut pinout!(p.wf_mosi),
            Default::default(),
        ),
        &mut pinout!(p.wf_cs),
        &mut pinout!(p.wf_fod),
    ).await;
    let mut near = Paj7025::new(
        Spim::new(
            &mut pinout!(p.nf_spim),
            Irqs,
            &mut pinout!(p.nf_sck),
            &mut pinout!(p.nf_miso),
            &mut pinout!(p.nf_mosi),
            Default::default(),
        ),
        &mut pinout!(p.nf_cs),
        &mut pinout!(p.nf_fod),
    ).await;

    wide.set_gain_1(0).await;
    wide.set_gain_2(0).await;
    wide.set_frame_period(16384).await;
    wide.set_exposure_time(2048).await;
    wide.set_bank1_sync_updated(1).await;
    near.set_gain_1(0).await;
    near.set_gain_2(0).await;
    near.set_frame_period(16384).await;
    near.set_exposure_time(2048).await;
    near.set_bank1_sync_updated(1).await;
    near.set_brightness_threshold(100).await;
    near.set_noise_threshold(15).await;
    wide.set_brightness_threshold(100).await;
    wide.set_noise_threshold(15).await;
    near.set_resolution_x(4095).await;
    near.set_resolution_y(4095).await;
    wide.set_resolution_x(4095).await;
    wide.set_resolution_y(4095).await;
    embassy_time::Timer::after_millis(100).await;

    info!("wide.exposure_time = {}", wide.exposure_time().await);
    info!("wide.frame_period = {}", wide.frame_period().await);
    info!("near.exposure_time = {}", near.exposure_time().await);
    info!("near.frame_period = {}", near.frame_period().await);
    // Run the USB device.
    let (mut class, usb) = usb_device(p.USBD);
    spawner.must_spawn(run_usb(usb));
    class.wait_connection().await;

    // Wait for something to get sent
    let feature_buf: &mut [u8; 256] = shared_buffers[0][..256].as_mut().try_into().unwrap();
    loop {
        let v = wait_for_serial(&mut class).await;
        if v == b'a' {
            break;
        } else {
            // only read from wide because i'm lazy
            wide.trigger_fod().await;
            embassy_time::Timer::after_millis(1000/200).await;
            wide.set_bank(0x05).await;
            wide.read_register_array(0x00, feature_buf).await;
            write_serial(&mut class, feature_buf).await;
        }
    }

    info!("Transitioning to image mode");
    join(wide.image_mode(), near.image_mode()).await;

    let mut config = spis::Config::default();
    config.mode = spis::MODE_3;
    let wide_spis = Spis::new_rxonly(
        &mut pinout!(p.wf_spis),
        Irqs,
        &mut pinout!(p.wf_cs),
        &mut pinout!(p.wf_sck),
        &mut pinout!(p.wf_mosi),
        config,
    );
    let mut config = spis::Config::default();
    config.mode = spis::MODE_3;
    let near_spis = Spis::new_rxonly(
        &mut pinout!(p.nf_spis),
        Irqs,
        &mut pinout!(p.nf_cs),
        &mut pinout!(p.nf_sck),
        &mut pinout!(p.nf_mosi),
        config,
    );
    let nf_free_buffers = Channel::<NoopRawMutex, _, 1>::new();
    let wf_free_buffers = Channel::<NoopRawMutex, _, 1>::new();
    let image_buffers = Channel::<NoopRawMutex, _, NUM_BUFFERS>::new();
    let [b0, b1] = shared_buffers;
    nf_free_buffers.try_send(b0).unwrap();
    wf_free_buffers.try_send(b1).unwrap();
    let wide_loop = paj7025_image_loop(0, wide_spis, wf_free_buffers.receiver(), image_buffers.sender());
    let near_loop = paj7025_image_loop(1, near_spis, nf_free_buffers.receiver(), image_buffers.sender());
    let buffer_mgmt_loop = async {
        loop {
            let buf = if let Ok(b) = image_buffers.try_receive() {
                b
            } else {
                image_buffers.receive().await
            };
            let id = buf[buf.len()-3];
            write_serial(&mut class, buf).await;
            if id == 0 {
                wf_free_buffers.try_send(buf).unwrap();
            } else {
                nf_free_buffers.try_send(buf).unwrap();
            }
            let c = wait_for_serial(&mut class).await;
            if c == b'r' {
                cortex_m::peripheral::SCB::sys_reset();
            }
        }
    };

    // #[cfg(any(feature = "atslite-1-1", feature = "atslite-2-2"))]
    // spawner.must_spawn(power_button_loop(pinout!(p.pwr_btn).into()));

    join3(
        near_loop,
        wide_loop,
        buffer_mgmt_loop,
    ).await;
}

// #[cfg(any(feature = "atslite-1-1", feature="atslite-2-2"))]
// #[embassy_executor::task]
// async fn power_button_loop(pin: AnyPin) {
//     let mut pwr_btn = gpio::Input::new(pin, Pull::None);
//     pwr_btn.wait_for_falling_edge().await;
//     // cortex_m::peripheral::SCB::sys_reset();
//     info!("button pressed");
// }

async fn paj7025_image_loop<'b, T, M, const N: usize, const O: usize>(
    id: u8,
    mut spis: Spis<'_, T>,
    free_buffers: Receiver<'_, M, &'b mut [u8; 98*98 + 98*3], N>,
    image_buffers: Sender<'_, M, &'b mut [u8; 98*98 + 98*3], O>,
)
where
    T: spis::Instance,
    M: RawMutex,
{
    loop {
        let buffer = if let Ok(b) = free_buffers.try_receive() {
            b
        } else {
            free_buffers.receive().await
        };
        let result = spis.read(buffer).await;
        if let Ok(len) = result {
            let len16 = len as u16;
            let len_bytes = len16.to_le_bytes();
            // put the len bytes at the end because there is nothing there that we care about.
            buffer[98*98 + 98*3 - 3] = id;
            buffer[98*98 + 98*3 - 2] = len_bytes[0];
            buffer[98*98 + 98*3 - 1] = len_bytes[1];
        }
        image_buffers.try_send(buffer).unwrap();
    }
}

async fn write_serial<'d, D: embassy_usb::driver::Driver<'d>>(class: &mut CdcAcmClass<'d, D>, data: &[u8]) {
    let max_packet_size = usize::from(class.max_packet_size());
    for chunk in data.chunks(max_packet_size) {
        class.write_packet(chunk).await.unwrap();
    }
    if data.len() % usize::from(max_packet_size) == 0 {
        class.write_packet(&[]).await.unwrap();
    }
}

async fn wait_for_serial<'d, D: embassy_usb::driver::Driver<'d>>(class: &mut CdcAcmClass<'d, D>) -> u8 {
    loop {
        let mut buf = [0; 64];
        loop {
            let Ok(_n) = class.read_packet(&mut buf).await else { break };
            return buf[0];
        }
    }
}

#[embassy_executor::task]
async fn run_usb(mut device: UsbDevice<'static, embassy_nrf::usb::Driver<'static, embassy_nrf::peripherals::USBD, HardwareVbusDetect>>) -> ! {
    device.run().await
}

/// Panics if called more than once.
fn usb_device(p: impl Peripheral<P = peripherals::USBD> + 'static) -> (
    CdcAcmClass<'static, Driver<'static, peripherals::USBD, HardwareVbusDetect>>,
    UsbDevice<'static, usb::Driver<'static, peripherals::USBD, HardwareVbusDetect>>,
) {
    // Create the driver, from the HAL.
    let driver = Driver::new(p, Irqs, HardwareVbusDetect::new(Irqs));

    // Create embassy-usb Config
    let mut config = embassy_usb::Config::new(0xc0de, 0xcafe);
    config.manufacturer = Some("Embassy");
    config.product = Some("USB-serial example");
    config.serial_number = Some("12345678");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    // Required for windows compatibility.
    // https://developer.nordicsemi.com/nRF_Connect_SDK/doc/1.9.1/kconfig/CONFIG_CDC_ACM_IAD.html#help
    config.device_class = 0xEF;
    config.device_sub_class = 0x02;
    config.device_protocol = 0x01;
    config.composite_with_iads = true;

    // Create embassy-usb DeviceBuilder using the driver and config.
    // It needs some buffers for building the descriptors.
    static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static STATE: StaticCell<State> = StaticCell::new();

    let config_descriptor = CONFIG_DESCRIPTOR.init_with(|| [0; 256]);
    let bos_descriptor = BOS_DESCRIPTOR.init_with(|| [0; 256]);
    let msos_descriptor = MSOS_DESCRIPTOR.init_with(|| [0; 256]);
    let control_buf = CONTROL_BUF.init_with(|| [0; 64]);

    let state = STATE.init_with(State::new);
    let mut builder = Builder::new(
        driver,
        config,
        config_descriptor,
        bos_descriptor,
        msos_descriptor,
        control_buf,
    );

    // Create classes on the builder.
    let class = CdcAcmClass::new(&mut builder, state, 64);

    // Build the builder.
    let usb = builder.build();
    (class, usb)
}

struct Disconnected {}

impl From<EndpointError> for Disconnected {
    fn from(val: EndpointError) -> Self {
        match val {
            EndpointError::BufferOverflow => panic!("Buffer overflow"),
            EndpointError::Disabled => Disconnected {},
        }
    }
}
