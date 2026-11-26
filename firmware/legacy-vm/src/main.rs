#![no_main]
#![no_std]

use defmt_rtt as _;
use embassy_executor::Spawner;
use panic_probe as _;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    legacy_vm::startup::run(spawner).await;
}
