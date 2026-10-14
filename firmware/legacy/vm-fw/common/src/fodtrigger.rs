use core::marker::PhantomData;

use embassy_nrf::Peri;
use embassy_nrf::gpio::{AnyPin, Level, OutputDrive};
use embassy_nrf::gpiote::{self, OutputChannel, OutputChannelPolarity};
use embassy_nrf::interrupt::typelevel::Interrupt;
use embassy_nrf::ppi::{self, Ppi};
use embassy_nrf::timer::{self, Instance, Timer};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;

pub static FOD_TICK_SIG: Signal<CriticalSectionRawMutex, ()> = Signal::new();

pub struct FodTrigger<'d, T: Instance> {
    timer: Timer<'d>,
    ppi_set: Ppi<'d, ppi::AnyConfigurableChannel, 1, 2>,
    ppi_clr: Ppi<'d, ppi::AnyConfigurableChannel, 1, 2>,
    _nf_ch: gpiote::OutputChannel<'d>,
    _wf_ch: gpiote::OutputChannel<'d>,
    running: bool,
    _t: PhantomData<T>,
}

impl<'d, T: Instance> FodTrigger<'d, T> {
    #[allow(clippy::too_many_arguments)]
    pub fn new<C1: gpiote::Channel, C2: gpiote::Channel>(
        nf: Peri<'d, AnyPin>,
        wf: Peri<'d, AnyPin>,
        gpiote_nf_ch: Peri<'d, C1>,
        gpiote_wf_ch: Peri<'d, C2>,

        timer_peri: Peri<'d, T>,
        ppi_set_ch: Peri<'d, ppi::AnyConfigurableChannel>,
        ppi_clr_ch: Peri<'d, ppi::AnyConfigurableChannel>,

        freq: timer::Frequency,
        timer_interval_ms: u32,
        fod_hold_us: u32,
    ) -> Self {
        let nf_ch = OutputChannel::new(
            gpiote_nf_ch,
            nf,
            Level::Low,
            OutputDrive::Standard,
            OutputChannelPolarity::Set,
        );
        let wf_ch = OutputChannel::new(
            gpiote_wf_ch,
            wf,
            Level::Low,
            OutputDrive::Standard,
            OutputChannelPolarity::Set,
        );

        let mut timer = Timer::new(timer_peri);
        timer.set_frequency(freq);

        let regs = timer.regs();
        regs.events_compare(1).write(|w| *w = 0);
        regs.intenset().write(|w| w.0 = 1u32 << (16 + 1));

        let cc0 = timer.cc(0);
        let cc1 = timer.cc(1);

        let period_us = timer_interval_ms * 1000;
        cc0.write(fod_hold_us);
        cc1.write(period_us);
        cc1.short_compare_clear();

        let ppi_set = Ppi::new_one_to_two(ppi_set_ch, cc1.event_compare(), nf_ch.task_set(), wf_ch.task_set());
        let ppi_clr = Ppi::new_one_to_two(ppi_clr_ch, cc0.event_compare(), nf_ch.task_clr(), wf_ch.task_clr());

        Self {
            timer,
            ppi_set,
            ppi_clr,
            _nf_ch: nf_ch,
            _wf_ch: wf_ch,
            running: false,
            _t: PhantomData,
        }
    }

    pub async fn wait_tick(&self) {
        FOD_TICK_SIG.wait().await;
    }

    pub fn start(&mut self) {
        if self.running {
            return;
        }
        self.timer.cc(0).clear_events();
        self.timer.cc(1).clear_events();

        self.ppi_set.enable();
        self.ppi_clr.enable();

        unsafe {
            T::Interrupt::enable();
        }

        self.timer.clear();
        self.timer.start();
        self.running = true;
    }

    #[allow(dead_code)]
    pub fn stop(&mut self) {
        if !self.running {
            return;
        }
        self.timer.stop();
        self.ppi_set.disable();
        self.ppi_clr.disable();
        T::Interrupt::disable();
        self.running = false;
    }
}
