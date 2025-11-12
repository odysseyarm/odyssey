use ats_usb::packets::vm::Packet;
use futures::future::BoxFuture;
use odyssey_hub_common::device::Device;

/// Object-safe device link trait.
///
/// Async methods return `BoxFuture` so the trait can be used as a `dyn DeviceLink`.
pub trait DeviceLink: Send {
    fn device(&self) -> Device;

    /// Receive next packet from device. Returns a boxed future so the trait is object-safe.
    fn recv<'a>(&'a mut self) -> BoxFuture<'a, anyhow::Result<Packet>>;

    /// Send a packet to the device. Returns a boxed future so the trait is object-safe.
    fn send<'a>(&'a mut self, pkt: Packet) -> BoxFuture<'a, anyhow::Result<()>>;
}
