use embassy_futures::{
    join::join,
    select::{Either, select},
};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::Duration;
use embassy_usb::class::cdc_acm;
use pag7661qn::mode;
use static_cell::ConstStaticCell;

use crate::{
    device_id, usb::{wait_for_serial, write_serial}, CommonContext, Pag, PagInt, UsbDriver, PROTOCOL_VERSION
};

type Channel<T, const N: usize> = embassy_sync::channel::Channel<NoopRawMutex, T, N>;
type Sender<'a, T, const N: usize> = embassy_sync::channel::Sender<'a, NoopRawMutex, T, N>;
type Receiver<'a, T, const N: usize> = embassy_sync::channel::Receiver<'a, NoopRawMutex, T, N>;
type Img<'a> = &'a mut [u8; 320 * 240];

pub async fn image_mode(mut ctx: CommonContext) -> CommonContext {
    defmt::info!("Entering Image Mode");
    let mut pag = ctx.pag.switch_mode(mode::Image).await.unwrap();
    let [b1, b2] = &mut ctx.img.shared_image_buffers;
    let image_buffers = Channel::<_, 1>::new();
    let free_buffers = Channel::<_, 2>::new();
    let cancel = Channel::<(), 1>::new();
    free_buffers.try_send(b1).unwrap();
    free_buffers.try_send(b2).unwrap();

    let rcv = &mut ctx.usb_rcv;
    let snd = &mut ctx.usb_snd;
    join(
        serial_loop(
            rcv,
            snd,
            image_buffers.receiver(),
            free_buffers.sender(),
            cancel.sender(),
        ),
        image_loop(
            &mut pag,
            &mut ctx.pag_int,
            free_buffers.receiver(),
            image_buffers.sender(),
            cancel.receiver(),
        ),
    )
    .await;
    defmt::info!("Exiting Image Mode");

    drop(free_buffers);
    drop(image_buffers);
    ctx.pag = pag.as_dynamic_mode();
    ctx
}

async fn serial_loop<'a>(
    rcv: &mut cdc_acm::Receiver<'static, UsbDriver>,
    snd: &mut cdc_acm::Sender<'static, UsbDriver>,
    image_buffers: Receiver<'_, Img<'a>, 1>,
    free_buffers: Sender<'_, Img<'a>, 2>,
    cancel: Sender<'_, (), 1>,
) {
    loop {
        match wait_for_serial(rcv).await {
            b'a' => {
                let image = image_buffers.receive().await;
                let _ = embassy_time::with_timeout(Duration::from_secs(1), async {
                    write_serial(snd, image).await;
                    free_buffers.send(image).await;
                })
                .await;
            }

            b'i' => {
                write_serial(snd, &PROTOCOL_VERSION.to_le_bytes()).await;
                write_serial(snd, &device_id()[..6]).await;
                // sensor resolution
                let [a, b] = 320u16.to_le_bytes(); // width
                let [c, d] = 240u16.to_le_bytes(); // height

                // object resolution
                let [e, f, g, h] = (320u32 * (1 << 6)).to_le_bytes();
                let [i, j, k, l] = (240u32 * (1 << 6)).to_le_bytes();

                let buf = [a, b, c, d, e, f, g, h, i, j, k, l];
                write_serial(snd, &buf).await;
            }

            b'r' => {
                defmt::info!("exiting image mode");
                cancel.send(()).await;
                break;
            }

            cmd => defmt::error!("Unknown command '{=u8}'", cmd),
        }
    }
}

async fn image_loop<'a>(
    pag: &mut Pag<mode::Image>,
    pag_int: &mut PagInt,
    free_buffers: Receiver<'_, Img<'a>, 2>,
    image_buffers: Sender<'_, Img<'a>, 1>,
    exit: Receiver<'_, (), 1>,
) {
    loop {
        let Either::First(buf) = select(free_buffers.receive(), exit.receive()).await else {
            break;
        };
        pag.wait_for_image(pag_int, buf).await.unwrap();
        // the image is mirrored for some reason
        buf.chunks_mut(320).for_each(|l| l.reverse());
        let Either::First(_) = select(image_buffers.send(buf), exit.receive()).await else {
            break;
        };
    }
    defmt::info!("exiting inner image loop");
}

const NUM_BUFFERS: usize = 2;

/// Singleton to hold static resources.
pub struct Context {
    shared_image_buffers: &'static mut [[u8; 320 * 240]; NUM_BUFFERS],
}

impl Context {
    /// Can only be called once
    pub fn take() -> Self {
        static SHARED_BUFFERS: ConstStaticCell<[[u8; 320 * 240]; NUM_BUFFERS]> =
            ConstStaticCell::new([[0; 320 * 240]; NUM_BUFFERS]);
        Self {
            shared_image_buffers: SHARED_BUFFERS.take()
        }
    }
}
