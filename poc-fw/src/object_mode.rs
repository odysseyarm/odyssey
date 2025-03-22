use core::mem::{MaybeUninit, transmute};

use defmt::Format;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::Mutex as AsyncMutex};
use embassy_usb::{class::cdc_acm, driver::EndpointError};
use pag7661qn::mode;
use protodongers::{
    GeneralConfig, Packet, PacketData as P, Props, ReadRegisterResponse,
    slip::{SlipDecodeError, encode_slip_frame},
};
use static_cell::ConstStaticCell;

use crate::{CommonContext, UsbDriver, device_id, usb::write_serial};

#[allow(unreachable_code)]
pub async fn object_mode(mut ctx: CommonContext) -> CommonContext {
    defmt::info!("Entering Object Mode");
    let pag = ctx.pag.switch_mode(mode::Object).await.unwrap();
    let pag = AsyncMutex::<NoopRawMutex, _>::new(pag);

    let mut pkt_rcv = PacketReceiver::new(&mut ctx.usb_rcv, ctx.obj.packet_receive_buffer);
    loop {
        let pkt = match pkt_rcv.wait_for_packet().await {
            Err(WaitForPacketError::Usb(EndpointError::Disabled)) => {
                pkt_rcv.rcv.wait_connection().await;
                continue;
            }
            Err(e) => {
                defmt::error!("Failed to read packet: {}", e);
                continue;
            }
            Ok(p) => p,
        };

        defmt::info!("Got {}", pkt.ty());

        let response = match pkt.data {
            P::WriteRegister(_) => todo!(),
            P::ReadRegister(register) => Some(Packet {
                id: pkt.id,
                data: P::ReadRegisterResponse(ReadRegisterResponse {
                    bank: register.bank,
                    address: register.address,
                    data: 0,
                }),
            }),
            P::WriteConfig(_) => todo!(),
            P::ReadConfig() => Some(Packet {
                id: pkt.id,
                data: P::ReadConfigResponse(GeneralConfig::default()),
            }),
            P::ReadProps() => Some(Packet {
                id: pkt.id,
                data: P::ReadPropsResponse(Props {
                    uuid: device_id()[..6].try_into().unwrap(),
                    product_id: crate::usb::PID,
                }),
            }),
            P::ObjectReportRequest() => todo!(),
            P::StreamUpdate(_) => None, // TODO
            P::Ack() => None,
            P::ReadConfigResponse(_) => None,
            P::ReadPropsResponse(_) => None,
            P::ObjectReport(_) => None,
            P::CombinedMarkersReport(_) => None,
            P::AccelReport(_) => None,
            P::ImpactReport(_) => None,
            P::FlashSettings() => None,
            P::Vendor(_, _) => None,
            P::ReadRegisterResponse(_) => None,
        };

        if let Some(r) = response.as_ref() {
            write_packet(
                &mut ctx.usb_snd,
                r,
                ctx.obj.packet_serialize_buffer,
                ctx.obj.packet_slip_encode_buffer,
            )
            .await;
        }
    }

    ctx.pag = pag.into_inner().as_dynamic_mode();
    ctx
}

/// Singleton to hold static resources.
pub struct Context {
    packet_receive_buffer: &'static mut [u8; 1024],
    packet_serialize_buffer: &'static mut [u8; 1024],
    packet_slip_encode_buffer: &'static mut [u8; 2049],
}

impl Context {
    /// Can only be called once
    pub fn take() -> Self {
        Self {
            packet_receive_buffer: static_byte_buffer!(1024),
            packet_serialize_buffer: static_byte_buffer!(1024),
            packet_slip_encode_buffer: static_byte_buffer!(2049),
        }
    }
}

struct PacketReceiver<'a> {
    rcv: &'a mut cdc_acm::Receiver<'static, UsbDriver>,
    buf: &'a mut [u8; 1024],
    n: usize,
}

impl<'a> PacketReceiver<'a> {
    fn new(rcv: &'a mut cdc_acm::Receiver<'static, UsbDriver>, buf: &'a mut [u8; 1024]) -> Self {
        Self { rcv, buf, n: 0 }
    }

    async fn wait_for_packet(&mut self) -> Result<Packet, WaitForPacketError> {
        // ignore ff lolol
        // gonna replace with slip anyways
        while self.n < 3 {
            self.n += self.rcv.read_packet(&mut self.buf[self.n..]).await?;
        }
        let size = usize::from(u16::from_le_bytes([self.buf[1], self.buf[2]])) * 2;
        while self.n < size + 1 {
            self.n += self.rcv.read_packet(&mut self.buf[self.n..]).await?;
        }

        let pkt = Packet::parse(&mut &self.buf[1..size + 1]);
        self.buf.copy_within(size + 1..self.n, 0);
        self.n -= size + 1;
        Ok(pkt?)
    }
}

async fn write_packet(
    snd: &mut cdc_acm::Sender<'static, UsbDriver>,
    pkt: &Packet,
    packet_serialize_buffer: &mut [u8; 1024],
    packet_slip_encode_buffer: &mut [u8; 2049],
) {
    let ser_len = {
        let mut ser =
            unsafe { transmute::<&mut [u8], &mut [MaybeUninit<u8>]>(packet_serialize_buffer) };
        pkt.serialize_raw(&mut ser);
        ser.len()
    };
    let len = packet_serialize_buffer.len() - ser_len;
    let pkt_bytes = &packet_serialize_buffer[..len];

    let slip_frame = encode_slip_frame(pkt_bytes, packet_slip_encode_buffer).unwrap();
    write_serial(snd, slip_frame).await;
}

#[derive(Format)]
enum WaitForPacketError {
    Slip(SlipDecodeError),
    Packet(protodongers::Error),
    Usb(EndpointError),
}

impl_from!(WaitForPacketError::Slip(SlipDecodeError));
impl_from!(WaitForPacketError::Packet(protodongers::Error));
impl_from!(WaitForPacketError::Usb(EndpointError));
