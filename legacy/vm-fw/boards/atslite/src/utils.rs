const MAJOR: u16 = match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_MAJOR"), 10) {
    Ok(v) => v,
    Err(_) => panic!("Invalid CARGO_PKG_VERSION_MAJOR"),
};
const MINOR: u16 = match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_MINOR"), 10) {
    Ok(v) => v,
    Err(_) => panic!("Invalid CARGO_PKG_VERSION_MINOR"),
};
const PATCH: u16 = match u16::from_str_radix(core::env!("CARGO_PKG_VERSION_PATCH"), 10) {
    Ok(v) => v,
    Err(_) => panic!("Invalid CARGO_PKG_VERSION_PATCH"),
};

pub const FIRMWARE_VERSION: [u16; 3] = [MAJOR, MINOR, PATCH];

pub fn device_id() -> [u8; 8] {
    let ficr = embassy_nrf::pac::FICR;
    let low = ficr.info().deviceid(0).read();
    let high = ficr.info().deviceid(1).read();
    let [a, b, c, d] = low.to_le_bytes();
    let [e, f, g, h] = high.to_le_bytes();
    [a, b, c, d, e, f, g, h]
}

pub fn set_pin_sense(
    port: &embassy_nrf::gpio::Port,
    idx: u8,
    sense: embassy_nrf::pac::gpio::vals::Sense,
) {
    use embassy_nrf::pac;
    let n = idx as usize;
    match port {
        embassy_nrf::gpio::Port::Port0 => pac::P0.pin_cnf(n).modify(|w| {
            w.set_dir(pac::gpio::vals::Dir::INPUT);
            w.set_input(pac::gpio::vals::Input::CONNECT);
            w.set_pull(pac::gpio::vals::Pull::PULLUP);
            w.set_drive(pac::gpio::vals::Drive::S0S1);
            w.set_sense(sense)
        }),
        embassy_nrf::gpio::Port::Port1 => pac::P1.pin_cnf(n).modify(|w| w.set_sense(sense)),
    }
}

#[macro_export]
macro_rules! battery_model_inc {
    (
        .param_1  = { $($p1:expr),* $(,)? },
        .temps    = { $($temps:expr),* $(,)? },
        .param_2  = { $($p2:expr),* $(,)? },
        .param_3  = { $($p3:expr),* $(,)? },
        .param_4  = { $($p4:expr),* $(,)? },
        .param_5  = { $($p5:expr),* $(,)? },
        .param_6  = { $($p6:expr),* $(,)? },
        .param_7  = { $($p7:expr),* $(,)? },
        .param_8  = { $($p8:expr),* $(,)? },
        .param_9  = { $($p9:expr),* $(,)? },
        .param_10 = { $($p10:expr),* $(,)? },
        .param_11 = { $($p11:expr),* $(,)? },
        .param_12 = { $($p12:expr),* $(,)? },
        name: $name:literal,
    ) => {
        mod __battery_model_generated {
            use nrfxlib_sys as sys;

            // Fixed sizes from the C struct
            pub static PARAM_1:  [f32; 201] = [ $($p1),* ];
            pub static TEMPS:    [f32;   3] = [ $($temps),* ];
            pub static PARAM_2:  [f32; 122] = [ $($p2),* ];
            pub static PARAM_3:  [f32; 201] = [ $($p3),* ];
            pub static PARAM_4:  [f32; 201] = [ $($p4),* ];
            pub static PARAM_5:  [f32; 201] = [ $($p5),* ];
            pub static PARAM_6:  [f32; 201] = [ $($p6),* ];
            pub static PARAM_7:  [f32; 122] = [ $($p7),* ];
            pub static PARAM_8:  [f32; 122] = [ $($p8),* ];
            pub static PARAM_9:  [f32;   3] = [ $($p9),* ];
            pub static PARAM_10: [f32;   6] = [ $($p10),* ];
            pub static PARAM_11: [f32;   6] = [ $($p11),* ];
            pub static PARAM_12: [f32;   3] = [ $($p12),* ];

            const fn pad_name(s: &str) -> [u8; 64] {
                let bs = s.as_bytes();
                let mut out = [0u8; 64];
                let mut i = 0;
                while i < bs.len() && i < 64 {
                    out[i] = bs[i];
                    i += 1;
                }
                out
            }
            pub static NAME: [u8; 64] = pad_name($name);

            pub static BATTERY_MODEL: sys::battery_model = sys::battery_model {
                param_1: PARAM_1,
                temps:   TEMPS,
                param_2: PARAM_2,
                param_3: PARAM_3,
                param_4: PARAM_4,
                param_5: PARAM_5,
                param_6: PARAM_6,
                param_7: PARAM_7,
                param_8: PARAM_8,
                param_9:  PARAM_9,
                param_10: PARAM_10,
                param_11: PARAM_11,
                param_12: PARAM_12,
                name: NAME,
            };
        }
        pub use __battery_model_generated::BATTERY_MODEL;
    };
}
