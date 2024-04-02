extern crate ats_usb;
extern crate hidapi;

use std::{convert::Infallible, net::{Ipv4Addr, SocketAddr}, time::Duration};

use iced::{
    futures::{Sink, SinkExt}, widget::{column, text, Column}
};
use odyssey_hub_server::server::run_server;
use tokio::net::UdpSocket;

fn main() -> iced::Result {
    iced::program("kajingo", Counter::update, Counter::view)
        .subscription(|_| {
            println!("subscription outer!");
            iced::subscription::channel(0, 4, |sender| {
                println!("subscription inner!");
                device_ping_task(sender)
            })
        })
        .subscription(|_| {
            println!("subscription outer!");
            iced::subscription::channel(1, 4, |sender| {
                println!("subscription inner!");
                device_hid_task(sender)
            })
        })
        .subscription(|_| {
            println!("subscription outer!");
            iced::subscription::channel(2, 4, |sender| {
                println!("subscription inner!");
                device_cdc_task(sender)
            })
        })
        .subscription(|_| {
            iced::subscription::channel(3, 0, |_| {
                async {
                    loop {
                        match run_server().await {
                            Ok(_) => (),
                            Err(e) => eprintln!("Error in run_server: {}", e),
                        }
                    }
                }
            })
        })
        .run()
}

#[derive(Default)]
struct Counter {
    device_list: Vec<SocketAddr>,
}

#[derive(Debug, Clone, Copy)]
enum Message {
    Connect(SocketAddr),
    Disconnect(SocketAddr),
}

impl Counter {
    pub fn view(&self) -> Column<Message> {
        let mut col = column![text("hello there")];
        for d in &self.device_list {
            col = col.push(text(d));
        }
        col
    }
    pub fn update(&mut self, message: Message) {
        match message {
            Message::Connect(a) => self.device_list.push(a),
            Message::Disconnect(a) => {
                let i = self.device_list.iter().position(|d| *d == a);
                if let Some(i) = i {
                    self.device_list.remove(i);
                }
            }
        }
    }
}

async fn device_ping_task(mut message_channel: impl Sink<Message> + Unpin) -> Infallible {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 23456)).await.unwrap();
    socket.set_broadcast(true).unwrap();

    let mut old_list = vec![];
    loop {
        let mut new_list = vec![];
        let mut buf = [0; 1472];
        socket.send_to(&[255, 1], ("10.0.0.255", 23456)).await.unwrap();
        futures::future::select(
            std::pin::pin!(tokio::time::sleep(Duration::from_secs(2))),
            std::pin::pin!(async {
                loop {
                    let (len, addr) = socket.recv_from(&mut buf).await.unwrap();
                    if buf[0] == 255 { continue; }
                    if !old_list.contains(&addr) {
                        let _ = message_channel.send(Message::Connect(addr)).await;
                    }
                    new_list.push(addr);
                }
            })
        ).await;
        dbg!(&new_list);
        for v in &old_list {
            if !new_list.contains(v) {
                let _ = message_channel.send(Message::Disconnect(*v)).await;
            }
        }
        old_list = new_list;
    }
}

async fn device_hid_task(mut message_channel: impl Sink<Message> + Unpin) -> Infallible {
    let api = hidapi::HidApi::new().unwrap();

    let mut old_list = vec![];
    loop {
        let mut new_list = vec![];
        for device in api.device_list() {
            if device.vendor_id() == 0x1915 && device.product_id() == 0x48AB {
                new_list.push(SocketAddr::new(Ipv4Addr::new(42, 0, 0, 1).into(), 00000));
            }
        }
        dbg!(&new_list);
        for v in &old_list {
            if !new_list.contains(v) {
                let _ = message_channel.send(Message::Disconnect(*v)).await;
            }
        }
        old_list = new_list;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn device_cdc_task(mut message_channel: impl Sink<Message> + Unpin) -> Infallible {
    let mut old_list = vec![];
    loop {
        let mut new_list = vec![];
        let ports = serialport::available_ports();
        let ports: Vec<_> = match ports {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to list serial ports {}", &e.to_string());
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        }.into_iter().filter(|port| {
            match &port.port_type {
                serialport::SerialPortType::UsbPort(port_info) => {
                    if port_info.vid != 0x1915 || port_info.pid != 0x520f {
                        return false;
                    }
                    if let Some(i) = port_info.interface {
                        // interface 0: cdc acm module
                        // interface 1: cdc acm module functional subordinate interface
                        // interface 2: cdc acm dfu
                        // interface 3: cdc acm dfu subordinate interface
                        i == 0
                    } else {
                        true
                    }
                },
                _ => false,
            }
        }).collect();
        for device in ports {
            new_list.push(SocketAddr::new(Ipv4Addr::new(42, 0, 0, 1).into(), 00000));
        }
        dbg!(&new_list);
        for v in &old_list {
            if !new_list.contains(v) {
                let _ = message_channel.send(Message::Disconnect(*v)).await;
            }
        }
        old_list = new_list;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
