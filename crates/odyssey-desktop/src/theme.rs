//! The brand palette from `docs/design/tokens.css`, for GPUI views.
//!
//! The token file stays the source of truth. The values below mirror it and
//! must change with it. The palette follows the system appearance, like the
//! stylesheet's `prefers-color-scheme` rule.

use gpui::{App, Global, Rgba, WindowAppearance};

/// `gpui::rgb` is not `const`, and the palettes below are.
const fn rgb(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Palette {
    pub canvas: Rgba,
    pub surface: Rgba,
    pub surface_alt: Rgba,
    pub surface_hover: Rgba,
    pub line: Rgba,
    pub line_strong: Rgba,
    pub fg: Rgba,
    pub fg_strong: Rgba,
    pub fg_muted: Rgba,
    pub fg_faint: Rgba,
    pub accent: Rgba,
    pub accent_strong: Rgba,
    pub accent_ink: Rgba,
    pub accent_soft: Rgba,
    pub on_accent: Rgba,
    pub success: Rgba,
    pub success_ink: Rgba,
    pub success_soft: Rgba,
    pub warning: Rgba,
    pub warning_ink: Rgba,
    pub warning_soft: Rgba,
    pub on_warning: Rgba,
    pub danger: Rgba,
    pub danger_ink: Rgba,
    pub danger_soft: Rgba,
    pub on_danger: Rgba,
    pub info: Rgba,
    pub info_ink: Rgba,
    pub info_soft: Rgba,
}

/// Corner radii from the tokens, in pixels.
pub(crate) const RADIUS_SM: f32 = 6.0;
pub(crate) const RADIUS: f32 = 10.0;
pub(crate) const RADIUS_LG: f32 = 14.0;
pub(crate) const RADIUS_XL: f32 = 22.0;

impl Palette {
    const LIGHT: Self = Self {
        canvas: rgb(0x00fa_f8f7),
        surface: rgb(0x00ff_ffff),
        surface_alt: rgb(0x00f1_eeed),
        surface_hover: rgb(0x00e9_e5e4),
        line: rgb(0x00e5_e0df),
        line_strong: rgb(0x00cf_c9c8),
        fg: rgb(0x002b_2727),
        fg_strong: rgb(0x0011_1111),
        fg_muted: rgb(0x005f_5957),
        fg_faint: rgb(0x0085_7f7e),
        accent: rgb(0x00ff_a44f),
        accent_strong: rgb(0x00ff_b36b),
        accent_ink: rgb(0x0099_4a00),
        accent_soft: rgb(0x00ff_e4c9),
        on_accent: rgb(0x0011_1111),
        success: rgb(0x001a_7f37),
        success_ink: rgb(0x0017_6f30),
        success_soft: rgb(0x00e3_f6e8),
        warning: rgb(0x00fb_bf24),
        warning_ink: rgb(0x008a_5a00),
        warning_soft: rgb(0x00ff_f4d6),
        on_warning: rgb(0x001c_1917),
        danger: rgb(0x00c4_302b),
        danger_ink: rgb(0x00b4_2823),
        danger_soft: rgb(0x00fd_e8e7),
        on_danger: rgb(0x00ff_ffff),
        info: rgb(0x000b_5cad),
        info_ink: rgb(0x000b_5cad),
        info_soft: rgb(0x00e5_f1ff),
    };
    const DARK: Self = Self {
        canvas: rgb(0x0027_2525),
        surface: rgb(0x002f_2c2c),
        surface_alt: rgb(0x001f_1d1d),
        surface_hover: rgb(0x003a_3636),
        line: rgb(0x0045_403f),
        line_strong: rgb(0x005f_5957),
        fg: rgb(0x00e5_e0df),
        fg_strong: rgb(0x00ff_ffff),
        fg_muted: rgb(0x00b5_b0af),
        fg_faint: rgb(0x0085_7f7e),
        accent: rgb(0x00ff_a44f),
        accent_strong: rgb(0x00ff_b36b),
        accent_ink: rgb(0x00ff_a44f),
        accent_soft: rgb(0x003d_2510),
        on_accent: rgb(0x0011_1111),
        success: rgb(0x002e_a043),
        success_ink: rgb(0x004a_c26b),
        success_soft: rgb(0x0016_301e),
        warning: rgb(0x00fb_bf24),
        warning_ink: rgb(0x00fb_bf24),
        warning_soft: rgb(0x003a_2e10),
        on_warning: rgb(0x001c_1917),
        danger: rgb(0x00c4_302b),
        danger_ink: rgb(0x00ff_7b72),
        danger_soft: rgb(0x003d_1a19),
        on_danger: rgb(0x00ff_ffff),
        info: rgb(0x006c_b6ff),
        info_ink: rgb(0x006c_b6ff),
        info_soft: rgb(0x0015_263b),
    };

    pub(crate) fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::DARK,
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::LIGHT,
        }
    }
}

/// Holds the palette for the system appearance. Views read it with
/// [`palette`], and the shell refreshes it when the appearance changes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ActivePalette(pub Palette);

impl Global for ActivePalette {}

/// A copy of the active palette, so a view holding it can still use its context mutably.
pub(crate) fn palette(cx: &App) -> Palette {
    cx.global::<ActivePalette>().0
}
