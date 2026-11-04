use embassy_nrf::Peripherals;
use embassy_nrf::config::{Config, HfclkSource, LfclkSource};

/// Configures nRF5340 regulators and clocks and returns the peripherals.
pub fn init() -> Peripherals {
    let mut config = Config::default();

    config.dcdc.regh = true;
    // The other DC/DC stages stay off: the erratum 160 workaround is not implemented.
    config.dcdc.regmain = false;
    config.dcdc.regradio = false;

    // The external crystals are more accurate than the internal RC oscillators.
    config.hfclk_source = HfclkSource::ExternalXtal;
    config.lfclk_source = LfclkSource::ExternalXtal;

    embassy_nrf::init(config)
}
