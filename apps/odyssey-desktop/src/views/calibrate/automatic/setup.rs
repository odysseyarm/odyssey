//! Step 1: what is being calibrated, which screen, which camera, and how big
//! it is (known from the display, typed in, or left to the camera).

use dioxus::prelude::*;
use odyssey_common::MAX_SCREEN_ID;

use super::camera_support::{self, Notice, SDK_DOWNLOAD_URL};
use super::illustrations::{ProjectorIllustration, TvIllustration};
use super::kind::DisplayKind;
use super::screens::screen_label;
use super::sdk_download::{DownloadSdk, DownloadedSdkRow};
use super::settings::{DIAGONAL_RANGE_IN, SizeError, SizeField, WIDTH_RANGE_CM};
use super::ui::{CARD, PRIMARY_BUTTON, PRIMARY_ID, SECONDARY_BUTTON, StepPage};
use super::wizard::{CameraList, Wizard};
use crate::components::ScreenPicker;
use crate::components::wizard::{Icon, IconView, Spinner};
use crate::sdk_install::{self, Delivery};

#[component]
pub fn SetupStep(wizard: Wizard) -> Element {
    // Cameras are only looked for while this step is on screen; the polling
    // stops when the component goes away.
    use_future(move || async move { wizard.poll_cameras().await });

    let can_continue = wizard.can_continue();
    let notes = wizard.notes.read().clone();

    rsx! {
        StepPage {
            title: "Let's calibrate a screen",
            subtitle: "A few quick questions, then we'll point the camera at the screen.",
            footer: rsx! {
                span {}
                button {
                    id: PRIMARY_ID,
                    class: PRIMARY_BUTTON,
                    r#type: "button",
                    disabled: !can_continue,
                    onclick: move |_| wizard.begin_position(),
                    "Continue"
                }
            },

            for note in notes {
                div {
                    key: "{note}",
                    role: "alert",
                    class: "rounded-xl border-2 border-amber-300 bg-amber-50 px-4 py-3 text-base text-amber-900 dark:border-amber-700 dark:bg-amber-950/60 dark:text-amber-100",
                    "{note}"
                }
            }

            section {
                class: "{CARD} flex flex-col gap-4",
                h2 { class: "text-xl font-semibold text-gray-900 dark:text-white", "1. What are you calibrating?" }
                KindChooser { wizard }
            }

            section {
                class: "{CARD} flex flex-col gap-4",
                h2 { class: "text-xl font-semibold text-gray-900 dark:text-white", "2. Which screen?" }
                ScreenPicker { selection: wizard.monitors }
                ScreenNumberPicker { wizard }
            }

            section {
                class: "{CARD} flex flex-col gap-3",
                h2 { class: "text-xl font-semibold text-gray-900 dark:text-white", "3. Which camera?" }
                CameraPicker { wizard }
                DownloadedSdkRow { wizard }
            }

            section {
                class: "{CARD} flex flex-col gap-3",
                h2 {
                    class: "text-xl font-semibold text-gray-900 dark:text-white",
                    match wizard.kind() {
                        DisplayKind::Flat => "4. How big is the screen?",
                        DisplayKind::Projector => "4. How big is the picture? (optional)",
                    }
                }
                SizeSection { wizard }
            }
        }
    }
}

#[component]
fn ScreenNumberPicker(wizard: Wizard) -> Element {
    let chosen = wizard.screen_id();
    let saved = *wizard.saved.read();
    rsx! {
        div {
            class: "flex flex-col gap-2",
            span {
                id: "screen-number-label",
                class: "text-base font-medium text-gray-500 dark:text-gray-400",
                "Save the result as:"
            }
            div {
                role: "radiogroup",
                aria_labelledby: "screen-number-label",
                class: "grid grid-cols-3 gap-2 sm:grid-cols-6",
                for id in 0..=MAX_SCREEN_ID {
                    {
                        let selected = id == chosen;
                        let taken = saved[usize::from(id)];
                        let class = if selected {
                            "border-blue-600 bg-blue-600 text-white"
                        } else {
                            "border-gray-300 bg-white text-gray-800 hover:border-blue-400 dark:border-gray-500 dark:bg-gray-800 dark:text-gray-100"
                        };
                        let status_class = if selected { "opacity-90" } else { "text-gray-500 dark:text-gray-400" };
                        rsx! {
                            button {
                                key: "{id}",
                                r#type: "button",
                                role: "radio",
                                aria_checked: selected,
                                class: "flex flex-col items-center rounded-lg border-2 px-2 py-2 focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:focus-visible:ring-blue-800 {class}",
                                onclick: move |_| *wizard.screen_choice.write() = Some(id),
                                span { class: "text-base font-semibold", "{screen_label(id)}" }
                                span {
                                    class: "text-xs {status_class}",
                                    if taken { "Calibrated" } else { "Empty" }
                                }
                            }
                        }
                    }
                }
            }
            if saved[usize::from(chosen)] {
                span {
                    class: "text-sm text-amber-700 dark:text-amber-300",
                    "{screen_label(chosen)} already has a calibration. You'll be asked before it is replaced."
                }
            }
        }
    }
}

#[component]
fn CameraPicker(wizard: Wizard) -> Element {
    let cameras = wizard.cameras.read().clone();
    let chosen = wizard.camera();

    match cameras {
        CameraList::Searching => rsx! {
            div {
                class: "flex items-center gap-3 text-base text-gray-600 dark:text-gray-300",
                Spinner { class: "size-5".to_string() }
                "Looking for cameras…"
            }
        },
        CameraList::Failed(error) => rsx! {
            NoCamera {
                title: "Couldn't look for cameras".to_string(),
                detail: "Unplug the camera and plug it back in. If this keeps happening, restart the app.".to_string(),
                technical: vec![error],
            }
        },
        CameraList::Unavailable(status) => match camera_support::notice(&status) {
            Some(notice) => rsx! { CameraSupportNotice { wizard, notice } },
            None => rsx! {},
        },
        CameraList::Found(list) if list.is_empty() => rsx! {
            NoCamera {
                title: "No camera found".to_string(),
                detail: "Plug in the Orbbec camera with its USB cable. This page notices by itself.".to_string(),
                technical: Vec::new(),
            }
        },
        CameraList::Found(list) => rsx! {
            div {
                role: "radiogroup",
                aria_label: "Camera",
                class: "flex flex-col gap-2",
                for camera in list {
                    {
                        let selected = chosen.as_ref().is_some_and(|c| c.id == camera.id);
                        let class = if selected {
                            "border-blue-600 bg-blue-50 dark:bg-blue-950/50 dark:border-blue-400"
                        } else {
                            "border-gray-300 bg-white hover:border-blue-400 dark:border-gray-500 dark:bg-gray-800"
                        };
                        let id = camera.id.clone();
                        rsx! {
                            button {
                                key: "{camera.id}",
                                r#type: "button",
                                role: "radio",
                                aria_checked: selected,
                                class: "flex items-center gap-3 rounded-lg border-2 px-4 py-3 text-left focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:focus-visible:ring-blue-800 {class}",
                                onclick: move |_| *wizard.camera_choice.write() = Some(id.clone()),
                                IconView { icon: Icon::Camera, class: "size-7 shrink-0 text-blue-600 dark:text-blue-400".to_string() }
                                span {
                                    class: "flex min-w-0 flex-col",
                                    span { class: "text-lg font-medium text-gray-900 dark:text-white", "{camera.name}" }
                                    if !camera.serial.is_empty() {
                                        span { class: "text-sm text-gray-500 dark:text-gray-400", "Serial {camera.serial}" }
                                    }
                                }
                                if selected {
                                    IconView { icon: Icon::Check, class: "ms-auto size-6 text-blue-600 dark:text-blue-400".to_string() }
                                }
                            }
                        }
                    }
                }
            }
        },
    }
}

#[component]
fn NoCamera(title: String, detail: String, technical: Vec<String>, children: Element) -> Element {
    rsx! {
        div {
            class: "flex flex-col items-center gap-2 rounded-xl border-2 border-dashed border-gray-300 px-4 py-6 text-center dark:border-gray-500",
            IconView { icon: Icon::Camera, class: "size-10 text-gray-400".to_string() }
            div { class: "text-xl font-semibold text-gray-900 dark:text-white", "{title}" }
            div { class: "text-base text-gray-600 dark:text-gray-300", "{detail}" }
            {children}
            if !technical.is_empty() {
                details {
                    class: "w-full text-left text-sm text-gray-500 dark:text-gray-400",
                    summary { class: "cursor-pointer text-center", "Details" }
                    div {
                        class: "mt-2 flex flex-col gap-1",
                        for (i, line) in technical.iter().enumerate() {
                            code { key: "{i}", class: "break-words", "{line}" }
                        }
                    }
                }
            }
        }
    }
}

/// The camera library is missing, broken or the wrong version. The page keeps
/// checking by itself; the button only saves the wait.
#[component]
fn CameraSupportNotice(wizard: Wizard, notice: Notice) -> Element {
    rsx! {
        NoCamera {
            title: notice.title,
            detail: notice.detail,
            technical: notice.details,
            div {
                class: "flex flex-wrap items-center justify-center gap-3 pt-2",
                if notice.offer_download {
                    match sdk_install::delivery() {
                        Delivery::Link => rsx! {
                            a { class: PRIMARY_BUTTON, href: SDK_DOWNLOAD_URL, "Get the Orbbec SDK" }
                        },
                        Delivery::Download => rsx! { DownloadSdk { wizard } },
                        Delivery::Installer => rsx! {
                            p {
                                class: "max-w-xl text-sm text-gray-600 dark:text-gray-300",
                                "This copy of Odyssey doesn't download software. To add Orbbec camera support, run the \
                                 Odyssey installer again, choose Change, and tick \"Orbbec camera support\"."
                            }
                        },
                    }
                }
                button {
                    r#type: "button",
                    class: SECONDARY_BUTTON,
                    onclick: move |_| wizard.request_camera_list(),
                    "Check again"
                }
            }
        }
    }
}

const KINDS: [DisplayKind; 2] = [DisplayKind::Flat, DisplayKind::Projector];

fn kind_title(kind: DisplayKind) -> &'static str {
    match kind {
        DisplayKind::Flat => "TV or monitor",
        DisplayKind::Projector => "Projector",
    }
}

fn kind_blurb(kind: DisplayKind) -> &'static str {
    match kind {
        DisplayKind::Flat => "A flat screen with a fixed size",
        DisplayKind::Projector => "A picture thrown onto a wall or screen",
    }
}

fn kind_id(kind: DisplayKind) -> &'static str {
    match kind {
        DisplayKind::Flat => "kind-flat",
        DisplayKind::Projector => "kind-projector",
    }
}

/// Two big illustrated cards behind one radio group. Arrow keys move the
/// choice, and only the chosen card is a tab stop, as for any radio group.
#[component]
fn KindChooser(wizard: Wizard) -> Element {
    let chosen = wizard.kind();
    rsx! {
        div {
            role: "radiogroup",
            aria_label: "What are you calibrating?",
            class: "grid grid-cols-1 gap-3 sm:grid-cols-2",
            onkeydown: move |e| {
                let step = match e.key() {
                    Key::ArrowRight | Key::ArrowDown => 1,
                    Key::ArrowLeft | Key::ArrowUp => KINDS.len() - 1,
                    _ => return,
                };
                e.prevent_default();
                let at = KINDS.iter().position(|&k| k == wizard.kind()).unwrap_or(0);
                let next = KINDS[(at + step) % KINDS.len()];
                wizard.set_kind(next);
                let _ = document::eval(&format!("document.getElementById('{}')?.focus();", kind_id(next)));
            },
            for kind in KINDS {
                {
                    let selected = kind == chosen;
                    let class = if selected {
                        "border-blue-600 bg-blue-50 ring-4 ring-blue-200 dark:border-blue-400 dark:bg-blue-950/50 dark:ring-blue-900"
                    } else {
                        "border-gray-300 bg-white hover:border-blue-400 dark:border-gray-500 dark:bg-gray-800"
                    };
                    rsx! {
                        button {
                            key: "{kind_id(kind)}",
                            id: kind_id(kind),
                            r#type: "button",
                            role: "radio",
                            aria_checked: selected,
                            tabindex: if selected { "0" } else { "-1" },
                            class: "relative flex flex-col items-center gap-2 rounded-2xl border-2 px-4 pb-4 pt-5 text-center transition-colors focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:focus-visible:ring-blue-800 {class}",
                            onclick: move |_| wizard.set_kind(kind),
                            if selected {
                                span {
                                    class: "absolute right-3 top-3 text-blue-600 dark:text-blue-300",
                                    IconView { icon: Icon::Check, class: "size-7".to_string() }
                                }
                            }
                            match kind {
                                DisplayKind::Flat => rsx! { TvIllustration { class: "h-28".to_string() } },
                                DisplayKind::Projector => rsx! { ProjectorIllustration { class: "h-28".to_string() } },
                            }
                            span { class: "text-xl font-semibold text-gray-900 dark:text-white", "{kind_title(kind)}" }
                            span { class: "text-base text-gray-600 dark:text-gray-300", "{kind_blurb(kind)}" }
                        }
                    }
                }
            }
        }
    }
}

/// What the size section says, by kind.
#[component]
fn SizeSection(wizard: Wizard) -> Element {
    match wizard.kind() {
        DisplayKind::Flat => rsx! { FlatSize { wizard } },
        DisplayKind::Projector => rsx! { ProjectorSize { wizard } },
    }
}

#[component]
fn FlatSize(wizard: Wizard) -> Element {
    let Some(finding) = wizard.finding.read().clone() else {
        return rsx! {
            div {
                class: "flex items-center gap-3 text-base text-gray-600 dark:text-gray-300",
                Spinner { class: "size-5".to_string() }
                "Reading the screen's size…"
            }
        };
    };
    let typed = !wizard.size_text.read().trim().is_empty();
    // The size field is always there when the display could not say its size,
    // and behind "Not right?" when it could.
    let field_open = !finding.is_found() || *wizard.manual_size.read();
    let advice = finding.advice();
    let aside = finding.projector_aside();
    let lines = finding.detail_lines();

    rsx! {
        div {
            class: "flex flex-col gap-3",
            div {
                class: "flex items-start gap-3",
                IconView {
                    icon: if finding.is_found() { Icon::Check } else { Icon::Viewfinder },
                    class: if finding.is_found() {
                        "mt-0.5 size-6 shrink-0 text-emerald-600 dark:text-emerald-400".to_string()
                    } else {
                        "mt-0.5 size-6 shrink-0 text-gray-500 dark:text-gray-400".to_string()
                    },
                }
                div {
                    class: "flex flex-col gap-1",
                    p {
                        class: "text-lg font-medium text-gray-900 dark:text-white",
                        if typed && finding.is_found() { "Using the size you entered instead of the TV's." } else { "{finding.headline()}" }
                    }
                    if let Some(advice) = advice {
                        p { class: "text-base text-gray-600 dark:text-gray-300", "{advice}" }
                    }
                    if let Some(aside) = aside {
                        p { class: "text-base text-gray-600 dark:text-gray-300", "{aside}" }
                    }
                }
            }

            if finding.is_found() && !field_open {
                div {
                    button {
                        class: "text-base font-medium text-blue-700 underline underline-offset-2 hover:text-blue-900 focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:text-blue-300 dark:hover:text-blue-100",
                        r#type: "button",
                        onclick: move |_| *wizard.manual_size.write() = true,
                        "Not right? Enter the size yourself"
                    }
                }
            }
            if field_open {
                SizeBox {
                    wizard,
                    label: if finding.is_found() { "Screen size (diagonal)" } else { "Size printed on the box (optional)" },
                }
                if finding.is_found() {
                    div {
                        button {
                            class: "text-base font-medium text-blue-700 underline underline-offset-2 hover:text-blue-900 focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:text-blue-300 dark:hover:text-blue-100",
                            r#type: "button",
                            onclick: move |_| {
                                wizard.size_text.write().clear();
                                *wizard.manual_size.write() = false;
                            },
                            "Use the TV's own size instead"
                        }
                    }
                }
            }

            div {
                class: "flex items-start gap-3 rounded-xl bg-sky-50 px-4 py-3 text-base text-sky-900 dark:bg-sky-950/50 dark:text-sky-100",
                IconView { icon: Icon::Bulb, class: "mt-0.5 size-6 shrink-0 text-sky-600 dark:text-sky-300".to_string() }
                span {
                    span { class: "font-semibold", "Before you start: " }
                    "set the TV to Game or PC mode, or turn on Just Scan, so it doesn't crop the picture."
                }
            }

            details {
                class: "text-sm text-gray-600 dark:text-gray-300",
                summary { class: "cursor-pointer select-none font-medium", "Details" }
                ul {
                    class: "mt-2 flex flex-col gap-1",
                    for line in lines {
                        li { key: "{line}", "{line}" }
                    }
                }
            }
        }
    }
}

#[component]
fn ProjectorSize(wizard: Wizard) -> Element {
    let field = wizard.field();
    rsx! {
        div {
            class: "flex flex-col gap-3",
            p {
                class: "text-lg font-medium text-gray-900 dark:text-white",
                "We'll measure the picture with the camera."
            }
            p {
                class: "text-base text-gray-600 dark:text-gray-300",
                "If you happen to know how big the picture is, type it in for a slightly more exact result. Otherwise leave it empty."
            }
            div {
                role: "radiogroup",
                aria_label: "Which size do you know?",
                class: "inline-flex w-fit overflow-hidden rounded-lg border border-gray-300 dark:border-gray-500",
                for (option, label) in [(SizeField::Diagonal, "Diagonal"), (SizeField::Width, "Width")] {
                    button {
                        key: "{label}",
                        r#type: "button",
                        role: "radio",
                        aria_checked: option == field,
                        class: if option == field {
                            "bg-blue-600 px-4 py-2 text-base font-medium text-white focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300"
                        } else {
                            "bg-white px-4 py-2 text-base font-medium text-gray-800 hover:bg-gray-100 focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:bg-gray-800 dark:text-gray-100 dark:hover:bg-gray-700"
                        },
                        onclick: move |_| {
                            if *wizard.size_field.peek() != option {
                                wizard.size_text.write().clear();
                                *wizard.size_field.write() = option;
                            }
                        },
                        "{label}"
                    }
                }
            }
            SizeBox {
                wizard,
                label: match field {
                    SizeField::Diagonal => "Picture size (diagonal)",
                    SizeField::Width => "Picture width",
                },
            }
        }
    }
}

/// The optional size box, in the unit of the field being asked for.
#[component]
fn SizeBox(wizard: Wizard, label: String) -> Element {
    let field = wizard.field();
    // A half-typed number such as the "4" of 40 is out of range for a moment;
    // that is reported once the tech leaves the box, not while they type.
    let mut left_box = use_signal(|| false);
    let error = wizard
        .entered()
        .err()
        .filter(|e| *left_box.read() || matches!(e, SizeError::NotANumber))
        .map(|e| e.message());
    let (unit, placeholder, range) = match field {
        SizeField::Diagonal => (
            "inches",
            "Measure it for me",
            format!("{}–{}", DIAGONAL_RANGE_IN.start(), DIAGONAL_RANGE_IN.end()),
        ),
        SizeField::Width => (
            "cm",
            "Measure it for me",
            format!("{}–{}", WIDTH_RANGE_CM.start(), WIDTH_RANGE_CM.end()),
        ),
    };
    rsx! {
        div {
            class: "flex flex-col gap-1",
            label {
                r#for: "size-input",
                class: "text-base font-medium text-gray-900 dark:text-white",
                "{label}"
            }
            div {
                class: "flex items-center gap-2",
                input {
                    id: "size-input",
                    r#type: "text",
                    inputmode: "decimal",
                    autocomplete: "off",
                    placeholder: "{placeholder}",
                    title: "{range} {unit}",
                    class: "w-48 rounded-lg border border-gray-300 bg-white px-3 py-2 text-lg text-gray-900 focus:border-blue-500 focus:outline-none focus:ring-4 focus:ring-blue-200 dark:border-gray-500 dark:bg-gray-800 dark:text-white dark:focus:ring-blue-900",
                    value: "{wizard.size_text}",
                    oninput: move |e| {
                        left_box.set(false);
                        *wizard.size_text.write() = e.value();
                    },
                    onblur: move |_| left_box.set(true),
                }
                span { class: "text-lg text-gray-600 dark:text-gray-300", "{unit}" }
            }
            if let Some(error) = error {
                span { role: "alert", class: "text-sm text-red-700 dark:text-red-300", "{error}" }
            }
        }
    }
}
