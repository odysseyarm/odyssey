#![no_std]
#![no_main]

use nrf_pac::uicr::vals::ApprotectPall;
use panic_probe as _;

#[cortex_m_rt::entry]
fn main() -> ! {
    let uicr = nrf_pac::UICR_NS;
    let a = uicr.approtect().read().pall();
    if a != ApprotectPall::UNPROTECTED {
        let nvmc = nrf_pac::NVMC_NS;
        nvmc.config()
            .write(|w| w.set_wen(nrf_pac::nvmc::vals::Wen::WEN));
        while !nvmc.ready().read().ready() {}
        uicr.approtect()
            .write(|a| a.set_pall(ApprotectPall::UNPROTECTED));
        while !nvmc.ready().read().ready() {}
        nvmc.config()
            .write(|w| w.set_wen(nrf_pac::nvmc::vals::Wen::REN));
        while !nvmc.ready().read().ready() {}
        cortex_m::peripheral::SCB::sys_reset();
    }
    nrf_pac::CTRLAP_NS
        .approtect()
        .disable()
        .write_value(ApprotectPall::UNPROTECTED.0);
    loop {
        cortex_m::asm::wfi();
    }
}
