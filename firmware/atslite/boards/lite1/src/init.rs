//! Board initialization

use embassy_nrf::{
    Peripherals,
    config::{Config, HfclkSource, LfclkSource},
};

/// Initialize the nRF5340 and return peripherals
pub fn init() -> Peripherals {
    let mut config = Config::default();

    // Enable DC/DC converter for nRF5340
    config.dcdc.regh = true;
    // Erratum 160 workaround is not implemented
    config.dcdc.regmain = false;
    config.dcdc.regradio = false;

    // Use external oscillators for better accuracy
    config.hfclk_source = HfclkSource::ExternalXtal;
    config.lfclk_source = LfclkSource::ExternalXtal;

    embassy_nrf::init(config)
}
