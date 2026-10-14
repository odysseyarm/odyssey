#![allow(clippy::needless_borrows_for_move)]

#[macro_export]
macro_rules! battery_model_inc {
    (
        name: $name:literal,
        // The include! expands here into the dotted Zephyr fields.
        .param_1 = { $($p1:expr),* $(,)? },
        .temps   = { $($temps:expr),* $(,)? },
        .param_2 = { $($p2:expr),* $(,)? },
        .param_3 = { $($p3:expr),* $(,)? },
        .param_4 = { $($p4:expr),* $(,)? },
        .param_5 = { $($p5:expr),* $(,)? },
        .param_6 = { $($p6:expr),* $(,)? },
        .param_7 = { $($p7:expr),* $(,)? },
        .param_8 = { $($p8:expr),* $(,)? },
        .param_9  = { $($p9:expr),*  $(,)? },
        .param_10 = { $($p10:expr),* $(,)? },
        .param_11 = { $($p11:expr),* $(,)? },
        .param_12 = { $($p12:expr),* $(,)? }
        $(, .name = $($_ignore_name_tokens:tt)* )?   // optional; ignored, we use `name:`
        $(,)?                                        // optional trailing comma
    ) => {
        mod __battery_model_generated {
            use core::ffi::c_char;
            use nrfxlib_sys as sys;

            // Keep slices (readable, no huge `static [f32; N]` boilerplate).
            pub static NAME_BYTES: &[u8] = $name.as_bytes();

            pub static TEMPS:   &[f32] = &[ $($temps),* ];
            pub static PARAM_1: &[f32] = &[ $($p1),* ];
            pub static PARAM_2: &[f32] = &[ $($p2),* ];
            pub static PARAM_3: &[f32] = &[ $($p3),* ];
            pub static PARAM_4: &[f32] = &[ $($p4),* ];
            pub static PARAM_5: &[f32] = &[ $($p5),* ];
            pub static PARAM_6: &[f32] = &[ $($p6),* ];
            pub static PARAM_7: &[f32] = &[ $($p7),* ];
            pub static PARAM_8: &[f32] = &[ $($p8),* ];
            pub static PARAM_9:  &[f32] = &[ $($p9),*  ];
            pub static PARAM_10: &[f32] = &[ $($p10),* ];
            pub static PARAM_11: &[f32] = &[ $($p11),* ];
            pub static PARAM_12: &[f32] = &[ $($p12),* ];

            // (Optional) a couple compile-time sanity checks that catch obvious mix-ups:
            const _: () = {
                // temps should be small (e.g., 3) but not zero
                assert!(TEMPS.len() > 0);
                // param_1 should be monotonic SoC points [0..1]; we can at least require > 1 entry
                assert!(PARAM_1.len() > 1);
            };

            pub static BATTERY_MODEL: sys::battery_model = sys::battery_model {
                // main curve grid + temps
                param_1: PARAM_1.as_ptr(),  param_1_len: PARAM_1.len() as u32,
                temps:   TEMPS.as_ptr(),    temps_len:   TEMPS.len() as u32,

                // coefficient tables
                param_2: PARAM_2.as_ptr(),
                param_3: PARAM_3.as_ptr(),
                param_4: PARAM_4.as_ptr(),
                param_5: PARAM_5.as_ptr(),
                param_6: PARAM_6.as_ptr(),
                param_7: PARAM_7.as_ptr(),
                param_8: PARAM_8.as_ptr(),

                // extra parameter blocks
                param_9:  PARAM_9.as_ptr(),
                param_10: PARAM_10.as_ptr(),
                param_11: PARAM_11.as_ptr(),
                param_12: PARAM_12.as_ptr(),

                // name (C expects ptr + len, not NUL-terminated)
                name: NAME_BYTES.as_ptr() as *const c_char,
                name_len: NAME_BYTES.len() as u32,
            };
        }

        // Re-export: `battery_model::BATTERY_MODEL`
        pub use __battery_model_generated::BATTERY_MODEL;
    };
}
