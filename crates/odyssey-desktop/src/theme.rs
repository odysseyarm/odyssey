//! The brand palette from `docs/design/tokens.css`, for GPUI views.
//!
//! The token file stays the source of truth; the values below mirror it and
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
        canvas: rgb(0xfaf8f7),
        surface: rgb(0xffffff),
        surface_alt: rgb(0xf1eeed),
        surface_hover: rgb(0xe9e5e4),
        line: rgb(0xe5e0df),
        line_strong: rgb(0xcfc9c8),
        fg: rgb(0x2b2727),
        fg_strong: rgb(0x111111),
        fg_muted: rgb(0x5f5957),
        fg_faint: rgb(0x857f7e),
        accent: rgb(0xffa44f),
        accent_strong: rgb(0xffb36b),
        accent_ink: rgb(0x994a00),
        accent_soft: rgb(0xffe4c9),
        on_accent: rgb(0x111111),
        success: rgb(0x1a7f37),
        success_ink: rgb(0x176f30),
        success_soft: rgb(0xe3f6e8),
        warning: rgb(0xfbbf24),
        warning_ink: rgb(0x8a5a00),
        warning_soft: rgb(0xfff4d6),
        on_warning: rgb(0x1c1917),
        danger: rgb(0xc4302b),
        danger_ink: rgb(0xb42823),
        danger_soft: rgb(0xfde8e7),
        on_danger: rgb(0xffffff),
        info: rgb(0x0b5cad),
        info_ink: rgb(0x0b5cad),
        info_soft: rgb(0xe5f1ff),
    };

    const DARK: Self = Self {
        canvas: rgb(0x272525),
        surface: rgb(0x2f2c2c),
        surface_alt: rgb(0x1f1d1d),
        surface_hover: rgb(0x3a3636),
        line: rgb(0x45403f),
        line_strong: rgb(0x5f5957),
        fg: rgb(0xe5e0df),
        fg_strong: rgb(0xffffff),
        fg_muted: rgb(0xb5b0af),
        fg_faint: rgb(0x857f7e),
        accent: rgb(0xffa44f),
        accent_strong: rgb(0xffb36b),
        accent_ink: rgb(0xffa44f),
        accent_soft: rgb(0x3d2510),
        on_accent: rgb(0x111111),
        success: rgb(0x2ea043),
        success_ink: rgb(0x4ac26b),
        success_soft: rgb(0x16301e),
        warning: rgb(0xfbbf24),
        warning_ink: rgb(0xfbbf24),
        warning_soft: rgb(0x3a2e10),
        on_warning: rgb(0x1c1917),
        danger: rgb(0xc4302b),
        danger_ink: rgb(0xff7b72),
        danger_soft: rgb(0x3d1a19),
        on_danger: rgb(0xffffff),
        info: rgb(0x6cb6ff),
        info_ink: rgb(0x6cb6ff),
        info_soft: rgb(0x15263b),
    };

    pub(crate) fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::DARK,
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::LIGHT,
        }
    }
}

/// Holds the palette for the system appearance. Views read it with
/// [`palette`]; the shell refreshes it when the appearance changes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ActivePalette(pub Palette);

impl Global for ActivePalette {}

pub(crate) fn palette(cx: &App) -> Palette {
    cx.global::<ActivePalette>().0
}
