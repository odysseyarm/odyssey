//! Status LED driven by the NPM1300's three LED outputs, one per RGB channel.

use core::pin::pin;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Timer};
use npm1300_rs::NPM1300;
use npm1300_rs::leds::LedMode;

static CMD_CH: Channel<CriticalSectionRawMutex, LedCmd, 4> = Channel::new();
static BLE_CONNECTED: AtomicBool = AtomicBool::new(false);

/// Since: Odyssey Desktop 0.7.0.
#[derive(Copy, Clone, Debug)]
pub enum LedIdx {
    /// Since: Odyssey Desktop 0.7.0.
    Ch0,
    /// Since: Odyssey Desktop 0.7.0.
    Ch1,
    /// Since: Odyssey Desktop 0.7.0.
    Ch2,
}

/// What the status LED shows.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum LedState {
    // Shown immediately.
    /// Since: Odyssey Desktop 0.7.0.
    Off,
    /// Since: Odyssey Desktop 0.7.0.
    TurningOff,
    /// Since: Odyssey Desktop 0.7.0.
    TurningOn,
    /// Since: Odyssey Desktop 0.7.0.
    ProgramError,
    // Fast blue blink.
    /// Since: Odyssey Desktop 0.7.0.
    Pairing,

    // Battery level, BLE disconnected.
    /// Since: Odyssey Desktop 0.7.0.
    BattCharging,
    /// Since: Odyssey Desktop 0.7.0.
    BattShutoff,
    /// Since: Odyssey Desktop 0.7.0.
    BattLow,
    /// Since: Odyssey Desktop 0.7.0.
    BattMed,
    /// Since: Odyssey Desktop 0.7.0.
    BattHigh,

    // Battery level, BLE connected.
    /// Since: Odyssey Desktop 0.7.0.
    BleBattCharging,
    /// Since: Odyssey Desktop 0.7.0.
    BleBattShutoff,
    /// Since: Odyssey Desktop 0.7.0.
    BleBattLow,
    /// Since: Odyssey Desktop 0.7.0.
    BleBattMed,
    /// Since: Odyssey Desktop 0.7.0.
    BleBattHigh,
}

impl LedState {
    /// `(on_ms, off_ms)` of one blink cycle. `off_ms == 0` means solid.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn frames(self) -> (u64, u64) {
        match self {
            LedState::TurningOff | LedState::TurningOn => (100, 150),
            LedState::Pairing => (200, 200),

            LedState::BattLow
            | LedState::BattMed
            | LedState::BattHigh
            | LedState::BleBattShutoff
            | LedState::BleBattLow
            | LedState::BleBattMed
            | LedState::BleBattHigh => (1000, 4000),

            // Solid: re-applied every 250 ms.
            LedState::Off | LedState::ProgramError | LedState::BattShutoff => (250, 0),

            LedState::BattCharging | LedState::BleBattCharging => (1000, 4000),
        }
    }

    /// Picks the battery state for `soc` in percent: charging wins, then 0 / ≤25 / ≤65 / above.
    ///
    /// Since: Odyssey Desktop 0.7.0.
    pub fn from_battery(charging: bool, soc: f32, ble_connected: bool) -> Self {
        if charging {
            if ble_connected {
                LedState::BleBattCharging
            } else {
                LedState::BattCharging
            }
        } else if soc <= 0.0 {
            if ble_connected {
                LedState::BleBattShutoff
            } else {
                LedState::BattShutoff
            }
        } else if soc <= 25.0 {
            if ble_connected {
                LedState::BleBattLow
            } else {
                LedState::BattLow
            }
        } else if soc <= 65.0 {
            if ble_connected {
                LedState::BleBattMed
            } else {
                LedState::BattMed
            }
        } else {
            if ble_connected {
                LedState::BleBattHigh
            } else {
                LedState::BattHigh
            }
        }
    }
}

/// Since: Odyssey Desktop 0.7.0.
pub const RGB_OFF: (bool, bool, bool) = (false, false, false);
/// Since: Odyssey Desktop 0.7.0.
pub const RGB_RED: (bool, bool, bool) = (true, false, false);
/// Since: Odyssey Desktop 0.7.0.
pub const RGB_GRN: (bool, bool, bool) = (false, true, false);
/// Since: Odyssey Desktop 0.7.0.
pub const RGB_BLU: (bool, bool, bool) = (false, false, true);
/// Since: Odyssey Desktop 0.7.0.
pub const RGB_YEL: (bool, bool, bool) = (true, true, false);
/// Since: Odyssey Desktop 0.7.0.
pub const RGB_WHT: (bool, bool, bool) = (true, true, true);

#[derive(Clone, Copy, Debug)]
enum LedCmd {
    Set(LedState),
    SetSeamless(LedState),
}

/// Sends commands to the LED animator. Once locked, the plain `set_state*` calls are ignored
/// until `unlock_state`.
///
/// Since: Odyssey Desktop 0.7.0.
#[derive(Debug)]
pub struct PmicLedsHandle {
    locked: AtomicBool,
}

impl PmicLedsHandle {
    /// Since: Odyssey Desktop 0.7.0.
    pub fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
        }
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub async fn set_state(&self, s: LedState) {
        if self.locked.load(Ordering::Relaxed) {
            return;
        }
        CMD_CH.send(LedCmd::Set(s)).await;
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub async fn set_state_seamless(&self, s: LedState) {
        if self.locked.load(Ordering::Relaxed) {
            return;
        }
        CMD_CH.send(LedCmd::SetSeamless(s)).await;
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub async fn lock_and_set_state(&self, s: LedState) {
        self.locked.store(true, Ordering::Relaxed);
        CMD_CH.send(LedCmd::Set(s)).await;
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub async fn lock_and_set_state_seamless(&self, s: LedState) {
        self.locked.store(true, Ordering::Relaxed);
        CMD_CH.send(LedCmd::SetSeamless(s)).await;
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub async fn unlock_state(&self) {
        self.locked.store(false, Ordering::Relaxed);
    }
}

impl Default for PmicLedsHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Sets the LED state even if a handle holds the lock.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn set_state_global(s: LedState) {
    let _ = CMD_CH.send(LedCmd::Set(s)).await;
}

/// Like [`set_state_global`], but keeps the current blink phase instead of restarting it.
///
/// Since: Odyssey Desktop 0.7.0.
pub async fn set_state_seamless_global(s: LedState) {
    let _ = CMD_CH.send(LedCmd::SetSeamless(s)).await;
}

/// Since: Odyssey Desktop 0.7.0.
pub fn set_ble_connected(connected: bool) {
    BLE_CONNECTED.store(connected, Ordering::Relaxed);
}

/// Since: Odyssey Desktop 0.7.0.
pub fn is_ble_connected() -> bool {
    BLE_CONNECTED.load(Ordering::Relaxed)
}

fn frame_durations(state: LedState) -> (Duration, Duration) {
    let (on, off) = state.frames();
    (Duration::from_millis(on), Duration::from_millis(off))
}

fn color_for(state: LedState, frame_idx: u8) -> (bool, bool, bool) {
    match state {
        LedState::Off => RGB_OFF,
        LedState::ProgramError => RGB_WHT,
        LedState::Pairing => {
            if frame_idx == 0 {
                RGB_BLU
            } else {
                RGB_OFF
            }
        }
        LedState::TurningOff => {
            if frame_idx == 0 {
                RGB_RED
            } else {
                RGB_OFF
            }
        }
        LedState::TurningOn => {
            if frame_idx == 0 {
                RGB_GRN
            } else {
                RGB_OFF
            }
        }
        LedState::BattCharging => RGB_RED,
        LedState::BattShutoff => RGB_RED,
        LedState::BattLow => {
            if frame_idx == 0 {
                RGB_RED
            } else {
                RGB_OFF
            }
        }
        LedState::BattMed => {
            if frame_idx == 0 {
                RGB_YEL
            } else {
                RGB_OFF
            }
        }
        LedState::BattHigh => {
            if frame_idx == 0 {
                RGB_GRN
            } else {
                RGB_OFF
            }
        }
        LedState::BleBattCharging => {
            if frame_idx == 0 {
                RGB_BLU
            } else {
                RGB_RED
            }
        }
        LedState::BleBattShutoff => RGB_RED,
        LedState::BleBattLow => {
            if frame_idx == 0 {
                RGB_RED
            } else {
                RGB_BLU
            }
        }
        LedState::BleBattMed => {
            if frame_idx == 0 {
                RGB_YEL
            } else {
                RGB_BLU
            }
        }
        LedState::BleBattHigh => {
            if frame_idx == 0 {
                RGB_GRN
            } else {
                RGB_BLU
            }
        }
    }
}

fn frames_in_state(state: LedState) -> u8 {
    let (_, off) = state.frames();
    if off == 0 { 1 } else { 2 }
}

/// Drives the PMIC LED outputs according to the current [`LedState`].
///
/// Since: Odyssey Desktop 0.7.0.
pub struct PmicLedAnimator<'a, I2c, Del>
where
    I2c: embedded_hal_async::i2c::I2c,
    Del: embedded_hal_async::delay::DelayNs,
{
    pmic: &'a Mutex<NoopRawMutex, NPM1300<I2c, Del>>,
    red: LedIdx,
    green: LedIdx,
    blue: LedIdx,
}

impl<'a, I2c, Del> PmicLedAnimator<'a, I2c, Del>
where
    I2c: embedded_hal_async::i2c::I2c,
    Del: embedded_hal_async::delay::DelayNs,
{
    /// Since: Odyssey Desktop 0.7.0.
    pub async fn new(
        pmic: &'a Mutex<NoopRawMutex, NPM1300<I2c, Del>>,
        red: LedIdx,
        green: LedIdx,
        blue: LedIdx,
    ) -> Result<(Self, PmicLedsHandle), npm1300_rs::NPM1300Error<I2c::Error>> {
        {
            let mut pmic = pmic.lock().await;
            pmic.configure_led0_mode(LedMode::Host).await?;
            pmic.configure_led1_mode(LedMode::Host).await?;
            pmic.configure_led2_mode(LedMode::Host).await?;
        }
        let mut me = Self {
            pmic,
            red,
            green,
            blue,
        };
        let _ = me.turn_rgb(false, false, false).await;
        Ok((me, PmicLedsHandle::new()))
    }

    /// Since: Odyssey Desktop 0.7.0.
    pub async fn run(&mut self) -> ! {
        let mut state = LedState::Off;
        let mut frame_idx = 0u8;

        let (mut on_dur, mut off_dur) = frame_durations(state);
        let mut frame_len = if frame_idx == 0 { on_dur } else { off_dur };
        let mut frame_start = Instant::now();

        self.apply_frame(state, frame_idx).await;

        loop {
            let elapsed = Instant::now() - frame_start;
            let remaining = if frame_len > elapsed {
                frame_len - elapsed
            } else {
                Duration::from_millis(0)
            };

            let wait_timer = Timer::after(remaining);
            let recv_cmd = CMD_CH.receive();

            let mut wait_timer = pin!(wait_timer);
            let mut recv_cmd = pin!(recv_cmd);

            match select(&mut wait_timer, &mut recv_cmd).await {
                Either::First(()) => {
                    let nframes = frames_in_state(state);
                    if nframes > 1 {
                        frame_idx = (frame_idx + 1) % nframes;
                    }
                    (on_dur, off_dur) = frame_durations(state);
                    frame_len = if frame_idx == 0 { on_dur } else { off_dur };
                    frame_start = Instant::now();
                    self.apply_frame(state, frame_idx).await;
                }

                Either::Second(cmd) => match cmd {
                    LedCmd::Set(new) => {
                        state = new;
                        frame_idx = 0;
                        (on_dur, _) = frame_durations(state);
                        frame_len = on_dur;
                        frame_start = Instant::now();
                        self.apply_frame(state, frame_idx).await;
                    }
                    LedCmd::SetSeamless(new) => {
                        state = new;
                        (on_dur, off_dur) = frame_durations(state);
                        self.apply_frame(state, frame_idx).await;
                        frame_len = if frame_idx == 0 { on_dur } else { off_dur };
                    }
                },
            }
        }
    }

    async fn apply_frame(&mut self, state: LedState, frame_idx: u8) {
        let (r, g, b) = color_for(state, frame_idx);
        let _ = self.turn_rgb(r, g, b).await;
    }

    async fn turn_rgb(
        &mut self,
        r: bool,
        g: bool,
        b: bool,
    ) -> Result<(), npm1300_rs::NPM1300Error<I2c::Error>> {
        self.set_chan(self.red, r).await?;
        self.set_chan(self.green, g).await?;
        self.set_chan(self.blue, b).await?;
        Ok(())
    }

    async fn set_chan(
        &mut self,
        ch: LedIdx,
        on: bool,
    ) -> Result<(), npm1300_rs::NPM1300Error<I2c::Error>> {
        let mut pmic = self.pmic.lock().await;
        match (ch, on) {
            (LedIdx::Ch0, true) => pmic.enable_led0().await,
            (LedIdx::Ch0, false) => pmic.disable_led0().await,
            (LedIdx::Ch1, true) => pmic.enable_led1().await,
            (LedIdx::Ch1, false) => pmic.disable_led1().await,
            (LedIdx::Ch2, true) => pmic.enable_led2().await,
            (LedIdx::Ch2, false) => pmic.disable_led2().await,
        }
    }
}
