use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncWrite};
use interprocess::{local_socket::{traits::tokio::Listener, ListenerOptions, NameTypeSupport, ToFsName, ToNsName}, os::windows::{local_socket::ListenerOptionsExt, AsSecurityDescriptorMutExt, SecurityDescriptor}};
use tokio::sync::mpsc;
use odyssey_hub_service_interface::{service_server::{Service, ServiceServer}, DeviceListReply, DeviceListRequest};
use tonic::transport::server::Connected;

use crate::device_tasks::device_tasks;

#[derive(Debug, Default)]
struct Server {
    devices: Vec<crate::device_tasks::Device>,
}

#[tonic::async_trait]
impl Service for Server {
    async fn get_device_list(
        &self,
        request: tonic::Request<DeviceListRequest>,
    ) -> Result<tonic::Response<DeviceListReply>, tonic::Status> {
        let devices = self.devices.iter().map(|d| {
            match d {
                crate::device_tasks::Device::Udp(addr) => odyssey_hub_service_interface::Device { device_oneof: Some(odyssey_hub_service_interface::device::DeviceOneof::UdpDevice(odyssey_hub_service_interface::UdpDevice { ip: addr.to_string(), port: addr.port() as i32 })) },
                crate::device_tasks::Device::Hid => odyssey_hub_service_interface::Device { device_oneof: Some(odyssey_hub_service_interface::device::DeviceOneof::HidDevice(odyssey_hub_service_interface::HidDevice { path: String::new() })) },
                crate::device_tasks::Device::Cdc => odyssey_hub_service_interface::Device { device_oneof: Some(odyssey_hub_service_interface::device::DeviceOneof::CdcDevice(odyssey_hub_service_interface::CdcDevice { path: String::new() })) },
            }
        }).collect::<Vec<odyssey_hub_service_interface::Device>>();

        let reply = DeviceListReply {
            devices,
        };

        Ok(tonic::Response::new(reply))
    }
}

pub enum Message {
    ServerInit(Result<(), std::io::Error>),
    Stop,
}

/// Wrapper around interprocess's `LocalSocketStream` to implement the `Connected` trait and
/// override `poll_flush`.
#[derive(Debug)]
pub struct LocalSocketStream(interprocess::local_socket::tokio::Stream);

impl LocalSocketStream {
    pub fn new(inner: interprocess::local_socket::tokio::Stream) -> Self {
        Self(inner)
    }
    fn inner_pin(self: Pin<&mut Self>) -> Pin<&mut interprocess::local_socket::tokio::Stream> {
        unsafe {
            self.map_unchecked_mut(|s| &mut s.0)
        }
    }
}

impl Connected for LocalSocketStream {
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {
    }
}

impl AsyncWrite for LocalSocketStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.inner_pin().poll_write(cx, buf)
    }

    fn poll_flush(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), std::io::Error>> {
        self.inner_pin().poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        self.inner_pin().poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }
}

impl AsyncRead for LocalSocketStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.inner_pin().poll_read(cx, buf)
    }
}

pub async fn run_service(sender: mpsc::UnboundedSender<Message>) -> anyhow::Result<()> {
    let name = {
        use NameTypeSupport as Nts;
        match NameTypeSupport::query() {
            Nts::OnlyFs => "/tmp/odyhub.sock".to_fs_name().unwrap(),
            Nts::OnlyNs | Nts::Both => "@odyhub.sock".to_ns_name().unwrap(),
        }
    };
    // Create our listener. In a more robust program, we'd check for an
    // existing socket file that has not been deleted for whatever reason,
    // ensure it's a socket file and not a normal file, and delete it.
	let mut sd = SecurityDescriptor::new()?;
	unsafe {
        sd.set_dacl(std::ptr::null_mut(), false)?;
	}
    let listener = ListenerOptions::new().security_descriptor(sd).name(name.clone()).create_tokio().unwrap();
    let listener = futures::stream::unfold((), |()| {
        async {
            let conn = listener.accept().await;
            dbg!(&conn);
            Some((conn.map(LocalSocketStream), ()))
        }
    });

    sender.send(Message::ServerInit(Ok(()))).unwrap();
    println!("Server running at {:?}", name);

    dbg!(tokio::select! {
        _ = device_tasks() => {},
        _ = tonic::transport::Server::builder()
            .add_service(ServiceServer::new(Server::default()))
            .serve_with_incoming(listener) => {},
    });

    Ok(())
}
