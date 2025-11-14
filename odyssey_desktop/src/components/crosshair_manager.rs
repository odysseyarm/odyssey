// src/components/crosshair_manager.rs

use std::sync::Once;

use dioxus::{
    desktop::{window, DesktopContext},
    html::geometry::euclid::Rect,
    logger::tracing,
    prelude::UnboundedReceiver,
    prelude::*,
};
use futures::StreamExt;
use tokio::sync::broadcast;

const CROSSHAIR_IMAGES: [Asset; 6] = [
    asset!("/assets/images/crosshairs/crosshair-red.png"),
    asset!("/assets/images/crosshairs/crosshair-blue.png"),
    asset!("/assets/images/crosshairs/crosshair-yellow.png"),
    asset!("/assets/images/crosshairs/crosshair-green.png"),
    asset!("/assets/images/crosshairs/crosshair-orange.png"),
    asset!("/assets/images/crosshairs/crosshair-gray.png"),
];
const BOOTSTRAP_SCRIPT: &str = include_str!("crosshair_canvas.js");
static BOOTSTRAP_ONCE: Once = Once::new();

#[derive(Clone, Debug)]
struct CrosshairUpdate {
    key: usize,
    x: f64,
    y: f64,
    image_src: String,
}

fn eval_script(desktop: &DesktopContext, script: &str) {
    if let Err(err) = desktop.webview.evaluate_script(script) {
        tracing::warn!("Crosshair canvas eval failed: {err}");
    }
}

fn ensure_bootstrap(desktop: &DesktopContext) {
    let cloned = desktop.clone();
    BOOTSTRAP_ONCE.call_once(move || {
        eval_script(&cloned, BOOTSTRAP_SCRIPT);
    });
}

fn resize_canvas(desktop: &DesktopContext, width: f64, height: f64) {
    ensure_bootstrap(desktop);
    eval_script(
        desktop,
        &format!(
            "window.__odysseyCrosshair?.resize({:.2}, {:.2});",
            width, height
        ),
    );
}

#[component]
pub fn CrosshairManager(hub: Signal<crate::hub::HubContext>) -> Element {
    let mut root_div = use_signal(|| None);
    let mut rect_signal = use_signal(Rect::zero);
    let mut known_devices = use_signal(Vec::<usize>::new);

    let desktop = window();
    ensure_bootstrap(&desktop);

    let crosshair_updates = use_coroutine({
        let desktop = desktop.clone();
        move |mut rx: UnboundedReceiver<CrosshairUpdate>| {
            let desktop = desktop.clone();
            async move {
                while let Some(update) = rx.next().await {
                    ensure_bootstrap(&desktop);
                    let script = format!(
                        "window.__odysseyCrosshair?.draw({key}, {x:.2}, {y:.2}, {src:?});",
                        key = update.key,
                        x = update.x,
                        y = update.y,
                        src = update.image_src
                    );
                    eval_script(&desktop, &script);
                }
            }
        }
    });

    {
        let hub = hub.clone();
        let rect_signal = rect_signal.clone();
        let crosshair_updates = crosshair_updates.clone();
        use_future(move || {
            let mut receiver = hub.peek().subscribe_tracking();
            async move {
                loop {
                    match receiver.recv().await {
                        Ok((device, tracking)) => {
                            let rect = rect_signal.read();
                            let width = rect.size.width as f64;
                            let height = rect.size.height as f64;
                            if width <= 0.0 || height <= 0.0 {
                                continue;
                            }

                            if let Some(key) = hub.peek().device_key(&device) {
                                let image_src = CROSSHAIR_IMAGES
                                    [key.min(CROSSHAIR_IMAGES.len() - 1)]
                                .to_string();
                                let x = tracking.aimpoint.x.clamp(0.0, 1.0) as f64 * width;
                                let y = tracking.aimpoint.y.clamp(0.0, 1.0) as f64 * height;
                                crosshair_updates.send(CrosshairUpdate {
                                    key,
                                    x,
                                    y,
                                    image_src,
                                });
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });
    }

    {
        let desktop = desktop.clone();
        let hub = hub.clone();
        let mut known_devices = known_devices.clone();
        use_effect(move || {
            let devices = (hub().devices)();
            let current_keys: Vec<usize> = devices.iter().map(|(key, _)| key).collect();
            let previous = known_devices.peek().clone();
            for key in previous.iter().filter(|k| !current_keys.contains(k)) {
                ensure_bootstrap(&desktop);
                eval_script(
                    &desktop,
                    &format!("window.__odysseyCrosshair?.clearKey({key});"),
                );
            }
            known_devices.set(current_keys);
        });
    }

    let desktop_for_mount = desktop.clone();
    let desktop_for_resize = desktop.clone();
    rsx! {
        div {
            class: "w-full h-full",
            onmounted: move |cx| {
                let desktop = desktop_for_mount.clone();
                async move {
                    let cx_data = cx.data();
                    if let Ok(rect) = cx_data.as_ref().get_client_rect().await {
                        resize_canvas(&desktop, rect.size.width as f64, rect.size.height as f64);
                        rect_signal.set(rect);
                    }
                    root_div.set(Some(cx_data));
                }
            },
            onresize: move |_| {
                let desktop = desktop_for_resize.clone();
                async move {
                    if let Some(root) = root_div() {
                        if let Ok(rect) = root.as_ref().get_client_rect().await {
                            resize_canvas(&desktop, rect.size.width as f64, rect.size.height as f64);
                            rect_signal.set(rect);
                        }
                    }
                }
            },
            canvas {
                id: "crosshair-layer",
                class: "absolute inset-0 pointer-events-none",
            }
        }
    }
}
