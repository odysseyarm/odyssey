use dioxus::prelude::*;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
// Shipped in the app, not fetched: the full installer never opens a network
// connection. Latin subset of Inter 4 from @fontsource-variable/inter 5.3.0
// (SIL OFL 1.1, assets/fonts/OFL.txt).
const INTER: Asset = asset!("/assets/fonts/inter-latin-wght-normal.woff2");

/// The stylesheet and brand font. Every window has its own document, so each
/// window's root component renders this once.
#[component]
pub(crate) fn Theme() -> Element {
    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        document::Style {
            "@font-face {{ font-family: 'Inter Variable'; src: url('{INTER}') format('woff2'); font-weight: 100 900; font-style: normal; font-display: swap; }}"
        }
    }
}
