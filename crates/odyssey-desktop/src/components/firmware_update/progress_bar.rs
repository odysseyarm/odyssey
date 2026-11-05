use dioxus::prelude::*;

#[component]
pub(super) fn ProgressBar(percent: usize) -> Element {
    rsx! {
        div {
            class: "progress",
            div {
                class: "progress-fill duration-100",
                style: "width: {percent}%",
            }
        }
    }
}
