//! Building blocks for step-by-step flows: a stepper, a big status banner,
//! labelled sliders and a live image with an overlay. They hold no flow logic,
//! so any wizard (the automatic calibration today, others later) can reuse
//! them.

use dioxus::prelude::*;

/// Shape of an icon, drawn on a 24-unit grid with a 1.5 stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Check,
    Camera,
    Viewfinder,
    Sun,
    Moon,
    MoveAway,
    MoveCloser,
    Cube,
    Rotate,
    FaceOn,
    Bulb,
    Warning,
    Cross,
    Crop,
}

impl Icon {
    pub fn path(self) -> &'static str {
        match self {
            Self::Check => "M9 12.75L11.25 15 15 9.75M21 12a9 9 0 11-18 0 9 9 0 0118 0z",
            Self::Camera => "M6.827 6.175A2.31 2.31 0 015.186 7.23c-.38.054-.757.112-1.134.175C2.999 7.58 2.25 8.507 2.25 9.574V18a2.25 2.25 0 002.25 2.25h15A2.25 2.25 0 0021.75 18V9.574c0-1.067-.75-1.994-1.802-2.169a47.865 47.865 0 00-1.134-.175 2.31 2.31 0 01-1.64-1.055l-.822-1.316a2.192 2.192 0 00-1.736-1.039 48.774 48.774 0 00-5.232 0 2.192 2.192 0 00-1.736 1.039l-.821 1.316zM16.5 12.75a4.5 4.5 0 11-9 0 4.5 4.5 0 019 0z",
            Self::Viewfinder => "M7.5 3.75H6A2.25 2.25 0 003.75 6v1.5M16.5 3.75H18A2.25 2.25 0 0120.25 6v1.5m0 9V18A2.25 2.25 0 0118 20.25h-1.5m-9 0H6A2.25 2.25 0 013.75 18v-1.5M15 12a3 3 0 11-6 0 3 3 0 016 0z",
            Self::Sun => "M12 3v2.25m6.364.386l-1.591 1.591M21 12h-2.25m-.386 6.364l-1.591-1.591M12 18.75V21m-4.773-4.227l-1.591 1.591M5.25 12H3m4.227-4.773L5.636 5.636M15.75 12a3.75 3.75 0 11-7.5 0 3.75 3.75 0 017.5 0z",
            Self::Moon => "M21.752 15.002A9.718 9.718 0 0118 15.75c-5.385 0-9.75-4.365-9.75-9.75 0-1.33.266-2.597.748-3.752A9.753 9.753 0 003 11.25C3 16.635 7.365 21 12.75 21a9.753 9.753 0 009.002-5.998z",
            Self::MoveAway => "M3.75 3.75v4.5m0-4.5h4.5m-4.5 0L9 9M3.75 20.25v-4.5m0 4.5h4.5m-4.5 0L9 15M20.25 3.75h-4.5m4.5 0v4.5m0-4.5L15 9m5.25 11.25h-4.5m4.5 0v-4.5m0 4.5L15 15",
            Self::MoveCloser => "M9 9V4.5M9 9H4.5M9 9L3.75 3.75M9 15v4.5M9 15H4.5M9 15l-5.25 5.25M15 9h4.5M15 9V4.5M15 9l5.25-5.25M15 15h4.5M15 15v4.5m0-4.5l5.25 5.25",
            Self::Cube => "M21 7.5l-9-5.25L3 7.5m18 0l-9 5.25m9-5.25v9l-9 5.25M3 7.5l9 5.25M3 7.5v9l9 5.25m0-9v9",
            Self::Rotate => "M16.023 9.348h4.992v-.001M2.985 19.644v-4.992m0 0h4.992m-4.993 0l3.181 3.183a8.25 8.25 0 0013.803-3.7M4.031 9.865a8.25 8.25 0 0113.803-3.7l3.181 3.182m0-4.991v4.99",
            Self::FaceOn => "M7.5 21L3 16.5m0 0L7.5 12M3 16.5h13.5m0-13.5L21 7.5m0 0L16.5 12M21 7.5H7.5",
            Self::Bulb => "M12 18v-5.25m0 0a6.01 6.01 0 001.5-.189m-1.5.189a6.01 6.01 0 01-1.5-.189m3.75 7.478a12.06 12.06 0 01-4.5 0m3.75 2.383a14.406 14.406 0 01-3 0M14.25 18v-.192c0-.983.658-1.823 1.508-2.316a7.5 7.5 0 10-7.517 0c.85.493 1.509 1.333 1.509 2.316V18",
            Self::Warning => "M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126zM12 15.75h.007v.008H12v-.008z",
            Self::Crop => "M6 3v13.5A1.5 1.5 0 007.5 18H21M3 6h13.5A1.5 1.5 0 0118 7.5V21",
            Self::Cross => "M9.75 9.75l4.5 4.5m0-4.5l-4.5 4.5M21 12a9 9 0 11-18 0 9 9 0 0118 0z",
        }
    }
}

/// How a banner is colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Good,
    Attention,
    Bad,
}

impl Tone {
    fn banner_class(self) -> &'static str {
        match self {
            Self::Good => "bg-emerald-50 border-emerald-300 text-emerald-900 dark:bg-emerald-950/60 dark:border-emerald-700 dark:text-emerald-100",
            Self::Attention => "bg-amber-50 border-amber-300 text-amber-900 dark:bg-amber-950/60 dark:border-amber-700 dark:text-amber-100",
            Self::Bad => "bg-red-50 border-red-300 text-red-900 dark:bg-red-950/60 dark:border-red-700 dark:text-red-100",
        }
    }

    fn icon_class(self) -> &'static str {
        match self {
            Self::Good => "text-emerald-600 dark:text-emerald-400",
            Self::Attention => "text-amber-600 dark:text-amber-400",
            Self::Bad => "text-red-600 dark:text-red-400",
        }
    }
}

#[component]
pub fn IconView(icon: Icon, class: Option<String>) -> Element {
    let class = class.unwrap_or_else(|| "size-6".into());
    rsx! {
        svg {
            class: "{class}",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.5",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d: icon.path() }
        }
    }
}

/// A large status message: an icon, a headline and one sentence of advice.
/// Announced politely to screen readers whenever it changes.
#[component]
pub fn StatusBanner(tone: Tone, icon: Icon, title: String, detail: String) -> Element {
    rsx! {
        div {
            role: "status",
            aria_live: "polite",
            class: "flex items-center gap-4 rounded-xl border-2 px-5 py-4 transition-colors {tone.banner_class()}",
            IconView { icon, class: "size-10 shrink-0 {tone.icon_class()}" }
            div {
                class: "min-w-0",
                div { class: "text-xl font-semibold leading-tight", "{title}" }
                div { class: "mt-0.5 text-base opacity-90", "{detail}" }
            }
        }
    }
}

/// The steps of a flow with the current one highlighted and finished ones
/// ticked. `current` may equal `steps.len()` once everything is done.
#[component]
pub fn Stepper(steps: Vec<&'static str>, current: usize) -> Element {
    let last = steps.len().saturating_sub(1);
    rsx! {
        ol {
            class: "flex items-center w-full",
            aria_label: "Progress",
            for (i, label) in steps.iter().copied().enumerate() {
                {
                    let done = i < current;
                    let active = i == current;
                    let dot = if done {
                        "bg-blue-600 text-white border-blue-600"
                    } else if active {
                        "bg-white text-blue-700 border-blue-600 ring-4 ring-blue-200 dark:bg-gray-800 dark:text-blue-300 dark:ring-blue-900"
                    } else {
                        "bg-white text-gray-400 border-gray-300 dark:bg-gray-800 dark:border-gray-600 dark:text-gray-500"
                    };
                    let text = if active {
                        "text-gray-900 dark:text-white font-semibold"
                    } else if done {
                        "text-gray-700 dark:text-gray-300"
                    } else {
                        "text-gray-400 dark:text-gray-500"
                    };
                    let bar = if done { "bg-blue-600" } else { "bg-gray-300 dark:bg-gray-600" };
                    rsx! {
                        li {
                            key: "{label}",
                            class: if i == last { "flex items-center" } else { "flex items-center flex-1" },
                            aria_current: if active { "step" } else { "false" },
                            div {
                                class: "flex items-center gap-2",
                                span {
                                    class: "flex size-8 shrink-0 items-center justify-center rounded-full border-2 text-sm font-semibold {dot}",
                                    if done {
                                        IconView { icon: Icon::Check, class: "size-5".to_string() }
                                    } else {
                                        "{i + 1}"
                                    }
                                }
                                span { class: "text-sm whitespace-nowrap {text}", "{label}" }
                            }
                            if i != last {
                                div { class: "mx-3 h-0.5 flex-1 rounded {bar}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A labelled range input. `oninput` fires continuously while dragging; callers
/// that restart something expensive debounce it themselves.
#[component]
pub fn Slider(
    id: String,
    label: String,
    value: u32,
    min: u32,
    max: u32,
    step: u32,
    /// Shown right of the label, e.g. "1500 µs".
    value_text: String,
    help: Option<String>,
    disabled: Option<bool>,
    oninput: EventHandler<u32>,
) -> Element {
    rsx! {
        div {
            class: "flex flex-col gap-1",
            div {
                class: "flex items-baseline justify-between",
                label { r#for: "{id}", class: "text-base font-medium text-gray-900 dark:text-white", "{label}" }
                span { class: "text-sm tabular-nums text-gray-600 dark:text-gray-300", "{value_text}" }
            }
            input {
                id: "{id}",
                r#type: "range",
                class: "w-full accent-blue-600 disabled:opacity-50",
                min: "{min}",
                max: "{max}",
                step: "{step}",
                value: "{value}",
                disabled: disabled.unwrap_or(false),
                oninput: move |e| {
                    if let Ok(v) = e.value().parse::<f64>() {
                        oninput.call(v.round().max(0.0) as u32);
                    }
                },
            }
            if let Some(help) = help {
                span { class: "text-sm text-gray-500 dark:text-gray-400", "{help}" }
            }
        }
    }
}

#[component]
pub fn Spinner(class: Option<String>) -> Element {
    let class = class.unwrap_or_else(|| "size-6".into());
    rsx! {
        svg {
            class: "animate-spin {class}",
            view_box: "0 0 24 24",
            fill: "none",
            role: "progressbar",
            "aria-label": "Working",
            circle { class: "opacity-25", cx: "12", cy: "12", r: "10", stroke: "currentColor", stroke_width: "4" }
            path { class: "opacity-90", fill: "currentColor", d: "M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" }
        }
    }
}

/// A picture with an SVG layer on top. The layer's coordinates are the
/// picture's own pixels (`width` by `height`), so callers draw detections
/// without scaling.
#[component]
pub fn OverlayImage(
    src: String,
    width: u32,
    height: u32,
    alt: String,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "relative w-full overflow-hidden rounded-xl bg-black shadow-inner",
            style: "aspect-ratio: {width} / {height};",
            if !src.is_empty() {
                img { class: "absolute inset-0 size-full object-fill", src: "{src}", alt: "{alt}" }
            }
            svg {
                class: "absolute inset-0 size-full",
                view_box: "0 0 {width} {height}",
                preserve_aspect_ratio: "none",
                {children}
            }
        }
    }
}
