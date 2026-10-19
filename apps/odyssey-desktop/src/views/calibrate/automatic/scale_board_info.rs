//! The "scale board" explanation: what the printed board is for, which one to
//! print and how, and the PDFs to print. Opened from Setup as a window over the
//! page; nothing in the calibration depends on it.

use std::path::PathBuf;

use dioxus::prelude::*;

use super::ui::SECONDARY_BUTTON;
use crate::scale_board::{self, Board};

const LINK: &str = "text-base font-medium text-blue-700 underline underline-offset-2 hover:text-blue-900 focus:outline-none focus-visible:ring-4 focus-visible:ring-blue-300 dark:text-blue-300 dark:hover:text-blue-100";
const HEADING: &str = "text-lg font-semibold text-gray-900 dark:text-white";
const BODY: &str = "text-base text-gray-700 dark:text-gray-200";

fn board_label(board: Board) -> &'static str {
    match board {
        Board::Letter => "US Letter (8.5 \u{d7} 11 in)",
        Board::A4 => "A4",
        Board::Rigid => "12 \u{d7} 8.5 in, for a print shop",
        Board::RigidLarge => "24 \u{d7} 17 in, for a print shop",
    }
}

/// What the last button press did.
#[derive(Clone, PartialEq)]
enum Done {
    Opened(PathBuf),
    Saved(PathBuf),
}
/// A small link under the size question that opens the explanation.
#[component]
pub fn ScaleBoardInfo() -> Element {
    let mut open = use_signal(|| false);
    rsx! {
        div {
            button {
                class: LINK,
                r#type: "button",
                onclick: move |_| open.set(true),
                "About the scale board (experimental)"
            }
        }
        if open() {
            ScaleBoardWindow { on_close: move |_| open.set(false) }
        }
    }
}

#[component]
fn ScaleBoardWindow(on_close: EventHandler<()>) -> Element {
    // What happened to the last PDF the tech asked for.
    let mut status = use_signal(|| None::<Result<Done, String>>);
    rsx! {
        div {
            class: "fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4",
            onclick: move |_| on_close.call(()),
            div {
                role: "dialog",
                aria_modal: "true",
                aria_labelledby: "scale-board-title",
                tabindex: 0,
                class: "flex max-h-full w-full max-w-3xl flex-col gap-5 overflow-y-auto rounded-xl bg-white p-6 shadow-xl outline-none dark:bg-gray-800",
                onclick: move |e| e.stop_propagation(),
                onmounted: move |e| async move {
                    let _ = e.set_focus(true).await;
                },
                onkeydown: move |e| {
                    if e.key() == Key::Escape {
                        on_close.call(());
                    }
                },

                div {
                    class: "flex items-start justify-between gap-4",
                    h2 { id: "scale-board-title", class: "text-2xl font-semibold text-gray-900 dark:text-white", "The scale board (experimental)" }
                    button {
                        class: SECONDARY_BUTTON,
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        "Close"
                    }
                }

                section {
                    class: "flex flex-col gap-2",
                    p {
                        class: BODY,
                        "A printed sheet of black dots, designed to a known size (you check your print against its scale bar, below). Stood next to the screen where the camera can see it, the camera can tell how far away the sheet is from how big it looks, and compare that with what its depth sensor says. If the two differ, the depth sensor's sense of scale is off."
                    }
                    p {
                        class: BODY,
                        "It's optional and it does not change your calibration: nothing in the wizard uses it yet. Odyssey support uses it with a separate test tool to check a camera's depth scale."
                    }
                }

                section {
                    class: "flex flex-col gap-3",
                    h3 { class: HEADING, "1. Pick a board and open its PDF" }
                    p {
                        class: BODY,
                        "Start with US Letter or A4: print it on paper and glue it flat on foam board. For the best result, have a print shop print the 12 \u{d7} 8.5 in file onto rigid board. The 24 \u{d7} 17 in one is only for when the test tool says the dots look too small at your distance."
                    }
                    div {
                        class: "flex flex-col gap-2",
                        for board in Board::ALL {
                            div {
                                key: "{board.file_name()}",
                                class: "flex flex-wrap items-center gap-3",
                                span { class: "min-w-64 {BODY}", "{board_label(board)}" }
                                button {
                                    class: SECONDARY_BUTTON,
                                    r#type: "button",
                                    onclick: move |_| {
                                        status.set(Some(scale_board::save_and_open(board).map(Done::Opened)));
                                    },
                                    "Open to print"
                                }
                                button {
                                    class: SECONDARY_BUTTON,
                                    r#type: "button",
                                    onclick: move |_| {
                                        spawn(async move {
                                            match scale_board::save_copy(board).await {
                                                Ok(Some(path)) => status.set(Some(Ok(Done::Saved(path)))),
                                                // Cancelled: leave whatever was shown.
                                                Ok(None) => {}
                                                Err(message) => status.set(Some(Err(message))),
                                            }
                                        });
                                    },
                                    "Save a copy\u{2026}"
                                }
                            }
                        }
                    }
                    match status() {
                        Some(Ok(Done::Opened(path))) => rsx! {
                            p { role: "status", class: "text-sm text-gray-600 dark:text-gray-300", "Opened {path.display()}. Print or save it from your PDF viewer." }
                        },
                        Some(Ok(Done::Saved(path))) => rsx! {
                            p { role: "status", class: "text-sm text-gray-600 dark:text-gray-300", "Saved {path.display()}." }
                        },
                        Some(Err(message)) => rsx! {
                            p { role: "alert", class: "text-sm text-red-700 dark:text-red-300", "{message}" }
                        },
                        None => rsx! {},
                    }
                }

                section {
                    class: "flex flex-col gap-2",
                    h3 { class: HEADING, "2. Print it" }
                    ul {
                        class: "flex list-disc flex-col gap-1 pl-6 {BODY}",
                        li { "Set the scale to Actual size or 100%. Never Fit, Shrink to printable area or Fit to page: that changes the size of everything." }
                        li { "Landscape, on the paper size the file is for. Black and white is fine; use the best quality." }
                        li { "Plain white matte paper. No gloss and no lamination: glare washes the dots out." }
                        li { "At a print shop, ask for one copy at 100%, no scaling, and ask whether they mount prints on foam board." }
                        li { "Glue it flat on 3/16 in foam board with spray adhesive or full-sheet double-sided tape, working from the middle out. Leave it flat under a weight for an hour. A wavy sheet spoils the measurement." }
                    }
                    p {
                        class: BODY,
                        "For a rigid print, a shop such as foamcoreprint.com (Gator Board Printing) can print the 12 \u{d7} 8.5 in file directly: custom size, 3/16 in thick, one side, no holes. Check their proof shows exactly that size, black dots on white, nothing cropped or scaled."
                    }
                }

                section {
                    class: "flex flex-col gap-2",
                    h3 { class: HEADING, "3. Check the scale of your print" }
                    p {
                        class: BODY,
                        "With a ruler or tape, measure the printed bar from the middle of its left end tick to the middle of its right end tick. It should be 200 mm (7.87 in); on the 24 \u{d7} 17 in board, 400 mm. If it isn't, don't fix the printer: the test tool takes the distance between dots, not the bar length, so work it out:"
                    }
                    p { class: "font-mono text-base text-gray-900 dark:text-white", "pitch = 40 \u{d7} measured bar \u{f7} 200   (24 \u{d7} 17 in board: 80 \u{d7} measured bar \u{f7} 400)" }
                    p {
                        class: BODY,
                        "For example, a bar that measures 201.4 mm means a pitch of 40.28 mm."
                    }
                }
            }
        }
    }
}
