#![no_std]
#![no_main]

const PROTOCOL_VERSION: u32 = 1;

mod imu;
mod init;
mod usb;

use core::str;

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_nrf::{
    gpio::{Input, Level, Output, OutputDrive, Pull},
    peripherals::{self, SPIM4},
    spim::{self, Spim},
};
use embassy_sync::{
    blocking_mutex::raw::NoopRawMutex,
    channel::{Channel, Receiver, Sender},
};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use pag7661qn::{Pag7661Qn, mode, spi::Pag7661QnSpi};
use static_cell::ConstStaticCell;
use usb::{wait_for_serial, write_serial};
use {defmt_rtt as _, panic_probe as _};

embassy_nrf::bind_interrupts!(struct Irqs {
    SERIAL1 => spim::InterruptHandler<peripherals::SERIAL1>;
    SPIM4 => spim::InterruptHandler<peripherals::SPIM4>;
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    USBREGULATOR => embassy_nrf::usb::vbus_detect::InterruptHandler;
});

const NUM_BUFFERS: usize = 2;
static SHARED_BUFFERS: ConstStaticCell<[[u8; 320 * 240]; NUM_BUFFERS]> =
    ConstStaticCell::new([[0; 320 * 240]; NUM_BUFFERS]);
type ImageBufferReceiver<const N: usize = NUM_BUFFERS> =
    Receiver<'static, NoopRawMutex, &'static mut [u8; 320 * 240], N>;
type ImageBufferSender<const N: usize = NUM_BUFFERS> =
    Sender<'static, NoopRawMutex, &'static mut [u8; 320 * 240], N>;
type ImageBufferChannel<const N: usize = NUM_BUFFERS> =
    Channel<NoopRawMutex, &'static mut [u8; 320 * 240], N>;
static FREE_BUFFERS: ConstStaticCell<ImageBufferChannel<2>> =
    ConstStaticCell::new(ImageBufferChannel::new());
static IMAGE_BUFFERS: ConstStaticCell<ImageBufferChannel<1>> =
    ConstStaticCell::new(ImageBufferChannel::new());

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let (_core_peripherals, p) = init::init();

    let (ref mut cdc_acm_class, usb) = usb::usb_device(p.USBD);
    spawner.must_spawn(usb::run_usb(usb));

    // Reset the PAG
    let mut vis_resetn = Output::new(p.P1_06, Level::Low, OutputDrive::Standard);
    // the datasheet doesn't say how long the reset pin needs to be asserted
    embassy_time::Timer::after_micros(500).await;
    vis_resetn.set_high();

    // Erratum 135
    unsafe {
        (0x5000ac04 as *mut u32).write_volatile(1);
    }
    let mut spim_config = spim::Config::default();
    spim_config.frequency = spim::Frequency::M16;
    spim_config.mode = spim::MODE_3;
    spim_config.bit_order = spim::BitOrder::MSB_FIRST;
    let spim = Spim::new(p.SPIM4, Irqs, p.P0_08, p.P0_10, p.P0_09, spim_config);
    let cs = Output::new(p.P0_11, Level::High, OutputDrive::Standard);
    let int_o = Input::new(p.P0_06, Pull::None);
    let Ok(spi_device) = ExclusiveDevice::new(spim, cs, Delay);
    let mut pag = Pag7661Qn::init_spi(spi_device, embassy_time::Delay, int_o, mode::Idle)
        .await
        .unwrap();

    pag.set_sensor_fps(30).await.unwrap();
    pag.set_sensor_exposure_us(true, 12700).await.unwrap();
    pag.set_sensor_gain(1).await.unwrap();
    let pag = pag.switch_mode(mode::Image).await.unwrap();

    cdc_acm_class.wait_connection().await;
    defmt::info!("CDC-ACM connected");

    let [b0, b1] = SHARED_BUFFERS.take();
    let free_buffers = FREE_BUFFERS.take();
    let image_buffers = IMAGE_BUFFERS.take();
    free_buffers.try_send(b0).unwrap();
    free_buffers.try_send(b1).unwrap();
    spawner.must_spawn(image_loop(
        pag,
        free_buffers.receiver(),
        image_buffers.sender(),
    ));

    join(
    imu::init(
        p.SERIAL1,
        p.P0_15.into(),
        p.P0_22.into(),
        p.P1_04.into(),
        p.P1_05.into(),
        p.P0_07.into(),
        p.P0_13.into(),
        p.TIMER0,
        p.PPI_CH0.into(),
        p.GPIOTE_CH0.into(),
    ),

    async { loop {
        match wait_for_serial(cdc_acm_class).await {
            b'a' => {
                let image = image_buffers.receive().await;
                write_serial(cdc_acm_class, image).await;
                free_buffers.send(image).await;
            }

            b'i' => {
                write_serial(cdc_acm_class, &PROTOCOL_VERSION.to_le_bytes()).await;
                write_serial(cdc_acm_class, &device_id()[..6]).await;
                // sensor resolution
                let [a, b] = 320u16.to_le_bytes(); // width
                let [c, d] = 240u16.to_le_bytes(); // height

                // object resolution
                let [e, f, g, h] = (320u32 * (1 << 6)).to_le_bytes();
                let [i, j, k, l] = (240u32 * (1 << 6)).to_le_bytes();

                let buf = [a, b, c, d, e, f, g, h, i, j, k, l];
                write_serial(cdc_acm_class, &buf).await;
            }

            cmd => defmt::error!("Unknown command '{=u8}'", cmd),
        }
    }},
    ).await;
}

#[embassy_executor::task]
async fn image_loop(
    mut pag: Pag7661Qn<
        Pag7661QnSpi<ExclusiveDevice<Spim<'static, SPIM4>, Output<'static>, Delay>>,
        embassy_time::Delay,
        Input<'static>,
        mode::Image,
    >,
    free_buffers: ImageBufferReceiver<2>,
    image_buffers: ImageBufferSender<1>,
) {
    loop {
        let buf = free_buffers.receive().await;
        pag.get_frame(buf).await.unwrap();
        // the image is mirrored for some reason
        buf.chunks_mut(320).for_each(|l| l.reverse());
        image_buffers.send(buf).await;
    }
}

fn device_id() -> [u8; 8] {
    let ficr = embassy_nrf::pac::FICR;
    let low = ficr.info().deviceid(0).read();
    let high = ficr.info().deviceid(1).read();
    let [a, b, c, d] = low.to_le_bytes();
    let [e, f, g, h] = high.to_le_bytes();
    [a, b, c, d, e, f, g, h]
}

fn device_id_str(buf: &mut [u8; 16]) -> &str {
    const CHARACTERS: [u8; 16] = *b"0123456789ABCDEF";
    let id = device_id();
    for (a, b) in id.into_iter().zip(buf.chunks_mut(2)) {
        b[0] = CHARACTERS[(a >> 4) as usize];
        b[1] = CHARACTERS[(a % 16) as usize];
    }
    unsafe { str::from_utf8_unchecked(buf) }
}
