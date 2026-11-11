// Assumes a little-endian target.
#![no_std]

#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "minicbor")]
mod cbor;
mod config;
mod device_name;
mod error;
mod list;
mod packet;
mod product_id;
#[cfg(kani)]
mod proofs;
mod props;
#[cfg(feature = "pyo3")]
mod py;
mod register;
mod reports;
mod stream_update;
mod usb_release;
mod version;

pub mod accessory;
pub mod control;
pub mod image;
pub mod legacy;
pub mod marker_control;
pub mod mux;
pub mod wire;

pub use config::{AccelConfig, ConfigKind, GeneralConfig, GyroConfig};
pub use device_name::{DeviceName, SetDeviceNameError, SetDeviceNameResult};
pub use error::Error;
pub use list::List;
pub use packet::{Packet, PacketData, PacketType, VendorData};
pub use product_id::ProductId;
pub use props::{PropKind, Props};
pub use register::{Port, ReadRegisterResponse, Register, WriteRegister};
pub use reports::{AccelReport, BatteryReport, ImpactReport, MarkersReport};
pub use stream_update::{StreamUpdate, StreamUpdateAction};
pub use usb_release::{BootloaderRelease, UsbDeviceRelease, UsbVersionReport};
pub use version::{DEVICE_PROTOCOL, Version};
