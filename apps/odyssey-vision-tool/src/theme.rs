//! The tool's look: a dark, Catppuccin Mocha based palette with roomier spacing
//! than egui's default, applied once at startup and shared by every window.

use eframe::egui::{self, Color32, CornerRadius, Stroke, vec2};

const BASE: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x2e);
const MANTLE: Color32 = Color32::from_rgb(0x18, 0x18, 0x25);
const CRUST: Color32 = Color32::from_rgb(0x11, 0x11, 0x1b);
const SURFACE0: Color32 = Color32::from_rgb(0x31, 0x32, 0x44);
const SURFACE1: Color32 = Color32::from_rgb(0x45, 0x47, 0x5a);
const SURFACE2: Color32 = Color32::from_rgb(0x58, 0x5b, 0x70);
const TEXT: Color32 = Color32::from_rgb(0xcd, 0xd6, 0xf4);
const SUBTEXT: Color32 = Color32::from_rgb(0xa6, 0xad, 0xc8);
const BLUE: Color32 = Color32::from_rgb(0x89, 0xb4, 0xfa);
const RED: Color32 = Color32::from_rgb(0xf3, 0x8b, 0xa8);
const YELLOW: Color32 = Color32::from_rgb(0xf9, 0xe2, 0xaf);

/// Corner radius shared by buttons, inputs and groups.
const ROUNDING: u8 = 6;

/// Installs the dark theme on `ctx`.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, visuals());
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = vec2(8.0, 8.0);
        style.spacing.button_padding = vec2(12.0, 6.0);
        style.spacing.interact_size.y = 26.0;
        style.spacing.window_margin = egui::Margin::same(12);
    });
}

/// Background of the error banner: the error color at low strength over the panel.
pub fn error_fill(visuals: &egui::Visuals) -> Color32 {
    visuals.panel_fill.lerp_to_gamma(RED, 0.15)
}

fn visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    let rounding = CornerRadius::same(ROUNDING);

    visuals.panel_fill = BASE;
    visuals.window_fill = BASE;
    visuals.extreme_bg_color = CRUST;
    visuals.faint_bg_color = MANTLE;
    visuals.code_bg_color = MANTLE;
    visuals.window_stroke = Stroke::new(1.0, SURFACE1);
    visuals.window_corner_radius = CornerRadius::same(ROUNDING + 2);
    visuals.hyperlink_color = BLUE;
    visuals.warn_fg_color = YELLOW;
    visuals.error_fg_color = RED;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = BLUE.gamma_multiply(0.35);
    visuals.selection.stroke = Stroke::new(1.0, BLUE);

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = BASE;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, SURFACE0);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, SUBTEXT);
    widgets.noninteractive.corner_radius = rounding;

    widgets.inactive.bg_fill = SURFACE0;
    widgets.inactive.weak_bg_fill = SURFACE0;
    widgets.inactive.bg_stroke = Stroke::NONE;
    widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.inactive.corner_radius = rounding;

    widgets.hovered.bg_fill = SURFACE1;
    widgets.hovered.weak_bg_fill = SURFACE1;
    widgets.hovered.bg_stroke = Stroke::new(1.0, SURFACE2);
    widgets.hovered.fg_stroke = Stroke::new(1.5, TEXT);
    widgets.hovered.corner_radius = rounding;

    widgets.active.bg_fill = SURFACE2;
    widgets.active.weak_bg_fill = SURFACE2;
    widgets.active.bg_stroke = Stroke::new(1.0, BLUE);
    widgets.active.fg_stroke = Stroke::new(2.0, TEXT);
    widgets.active.corner_radius = rounding;

    widgets.open.bg_fill = SURFACE0;
    widgets.open.weak_bg_fill = SURFACE0;
    widgets.open.corner_radius = rounding;

    visuals
}
