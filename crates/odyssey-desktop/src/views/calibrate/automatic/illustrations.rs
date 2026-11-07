//! Small original illustrations for the two kinds of display, drawn from the
//! palette so they follow the light or dark theme. The flat picture is one SVG
//! rasterised once per theme; the sparkles and the projector's beam are mono
//! SVGs laid over it so they can twinkle. Only the overlays animate.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, Image, ImageFormat, IntoElement, ParentElement as _,
    Rgba, Styled as _, div, img, px, svg,
};

use crate::components::wizard::with_alpha;
use crate::theme::Palette;

/// Brand colours that have no palette slot (`docs/design/tokens.css`).
const BRAND_OCHRE: u32 = 0x00c2_852c;
const BRAND_FLAME: u32 = 0x00ff_8344;

const TWINKLE: Duration = Duration::from_millis(2600);
const BEAM: Duration = Duration::from_millis(3200);

/// Drawn 4x the viewBox so the picture stays sharp at card sizes.
const SVG_OPEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 160 120" width="640" height="480" fill="none">"#;

macro_rules! mono {
    ($shape:literal) => {
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 160 120" width="640" height="480" fill="black">"#,
            $shape,
            "</svg>"
        )
        .as_bytes()
    };
}

const TV_SPARKLE: &[u8] =
    mono!(r#"<path d="M138 6 L141 15 L150 18 L141 21 L138 30 L135 21 L126 18 L135 15 Z"/>"#);
const TV_DOT: &[u8] = mono!(r#"<circle cx="22" cy="16" r="2.5"/>"#);
const PROJECTOR_SPARKLE: &[u8] =
    mono!(r#"<path d="M30 8 L32.5 15 L40 17.5 L32.5 20 L30 27 L27.5 20 L20 17.5 L27.5 15 Z"/>"#);
/// Starts at the lens's rim, so it never paints over the projector's body.
const PROJECTOR_BEAM: &[u8] = mono!(r#"<polygon points="65,55 98,22 98,84 65,68.3"/>"#);

fn byte(channel: f32) -> u8 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rounded 0..=255"
    )]
    let byte = (channel * 255.0).round() as u8;
    byte
}

fn hex(color: Rgba) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        byte(color.r),
        byte(color.g),
        byte(color.b)
    )
}

fn tv_svg(p: Palette) -> String {
    let (soft, faint, muted, accent) = (
        hex(p.accent_soft),
        hex(p.fg_faint),
        hex(p.fg_muted),
        hex(p.accent),
    );
    format!(
        r##"{SVG_OPEN}
<ellipse cx="80" cy="66" rx="70" ry="48" fill="{soft}"/>
<rect x="73" y="90" width="14" height="11" rx="2" fill="{faint}"/>
<rect x="50" y="100" width="60" height="7" rx="3.5" fill="{muted}"/>
<rect x="20" y="22" width="120" height="70" rx="9" fill="{muted}"/>
<rect x="26" y="28" width="108" height="58" rx="5" fill="{accent}"/>
<path d="M26 62 L72 28 H96 L26 80 Z" fill="white" fill-opacity="0.25"/>
<circle cx="68" cy="55" r="3" fill="white"/>
<circle cx="92" cy="55" r="3" fill="white"/>
<path d="M68 66 Q80 76 92 66" stroke="white" stroke-width="3.5" stroke-linecap="round"/>
</svg>"##
    )
}

fn projector_svg(p: Palette) -> String {
    let (soft, faint, muted, accent) = (
        hex(p.accent_soft),
        hex(p.fg_faint),
        hex(p.fg_muted),
        hex(p.accent),
    );
    let (surface, line, strong) = (hex(p.surface), hex(p.line_strong), hex(p.fg_strong));
    let mut dots = String::new();
    for y in [38, 52, 66] {
        for x in [110, 124, 138] {
            dots.push_str(&format!(
                r#"<circle cx="{x}" cy="{y}" r="3.2" fill="{strong}"/>"#
            ));
        }
    }
    format!(
        r##"{SVG_OPEN}
<ellipse cx="80" cy="66" rx="70" ry="48" fill="{soft}"/>
<rect x="98" y="20" width="52" height="66" rx="3" fill="{surface}"/>
<rect x="98" y="20" width="52" height="66" rx="3" stroke="{line}" stroke-width="2"/>
{dots}
<rect x="20" y="76" width="6" height="7" rx="1.5" fill="{faint}"/>
<rect x="46" y="76" width="6" height="7" rx="1.5" fill="{faint}"/>
<rect x="12" y="50" width="48" height="28" rx="9" fill="{muted}"/>
<circle cx="56" cy="64" r="9" fill="{faint}"/>
<circle cx="56" cy="64" r="5.5" fill="{accent}"/>
<circle cx="26" cy="60" r="2.4" fill="{surface}"/>
<circle cx="38" cy="60" r="2.4" fill="{surface}"/>
<path d="M26 68 Q32 73 38 68" stroke="{surface}" stroke-width="2.6" stroke-linecap="round"/>
</svg>"##
    )
}

/// The rasterised picture for `kind` in this theme, made once. The key holds
/// every colour the picture uses, so a theme switch gets its own copy.
fn picture(tv: bool, p: Palette) -> Arc<Image> {
    type Key = (bool, [u32; 7]);
    static CACHE: Mutex<Option<HashMap<Key, Arc<Image>>>> = Mutex::new(None);

    let pack = |c: Rgba| u32::from_be_bytes([0, byte(c.r), byte(c.g), byte(c.b)]);
    let key = (
        tv,
        [
            p.accent_soft,
            p.fg_faint,
            p.fg_muted,
            p.accent,
            p.surface,
            p.line_strong,
            p.fg_strong,
        ]
        .map(pack),
    );
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    cache
        .get_or_insert_with(HashMap::new)
        .entry(key)
        .or_insert_with(|| {
            let svg = if tv { tv_svg(p) } else { projector_svg(p) };
            Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()))
        })
        .clone()
}

/// A mono SVG over the whole picture that fades between `low` and 1 and back.
fn overlay(
    id: &'static str,
    data: &'static [u8],
    color: Rgba,
    period: Duration,
    low: f32,
    delay: f32,
) -> AnyElement {
    let fade = gpui::bounce(gpui::ease_in_out);
    svg()
        .data(data)
        .absolute()
        .inset_0()
        .size_full()
        .text_color(color)
        .with_animation(id, Animation::new(period).repeat(), move |el, delta| {
            el.opacity(low + (1.0 - low) * fade((delta + delay).fract()))
        })
        .into_any_element()
}

fn frame(height: f32, tv: bool, p: Palette, overlays: Vec<AnyElement>) -> impl IntoElement {
    div()
        .relative()
        .flex_shrink_0()
        .w(px(height * 4.0 / 3.0))
        .h(px(height))
        .child(img(picture(tv, p)).size_full())
        .children(overlays)
}

/// A friendly flat TV on a stand, with a sparkle.
pub(super) fn tv_illustration(p: Palette, height: f32) -> impl IntoElement {
    frame(
        height,
        true,
        p,
        vec![
            overlay(
                "tv-sparkle",
                TV_SPARKLE,
                gpui::rgb(BRAND_OCHRE),
                TWINKLE,
                0.35,
                0.0,
            ),
            overlay("tv-dot", TV_DOT, p.accent, TWINKLE, 0.35, 0.42),
        ],
    )
}

/// A projector throwing its beam onto a screen on the wall.
pub(super) fn projector_illustration(p: Palette, height: f32) -> impl IntoElement {
    frame(
        height,
        false,
        p,
        vec![
            overlay(
                "projector-beam",
                PROJECTOR_BEAM,
                with_alpha(p.accent, 0.5),
                BEAM,
                0.55,
                0.0,
            ),
            overlay(
                "projector-sparkle",
                PROJECTOR_SPARKLE,
                gpui::rgb(BRAND_FLAME),
                TWINKLE,
                0.35,
                0.0,
            ),
        ],
    )
}
