#![no_std]
#[cfg(test)]
extern crate std;

mod receiver;

pub use receiver::{Action, Receiver, Slots};
