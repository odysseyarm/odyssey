
/// Decoded tray icon, in whatever form the platform backend wants.
struct TrayImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn tray_image() -> TrayImage {
    let bytes: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/images/tray-icon.png"
    ));
    let image = image::load_from_memory(bytes)
        .expect("failed to decode the tray icon")
        .into_rgba8();
    let (width, height) = image.dimensions();
    TrayImage {
        width,
        height,
        rgba: image.into_raw(),
    }
}

/// Closing the window only hides it, so the tray is the sole way back.
fn show_window() {
    let window = dioxus::desktop::window();
    window.set_visible(true);
    window.set_focus();
}

/// Build the tray icon and wire up its menu.
///
/// Must be called from a component so the hooks run inside the Dioxus event
/// loop; the main App component is the natural place.
pub fn use_tray_menu(cancellation_token: tokio_util::sync::CancellationToken) {
    platform::use_tray(cancellation_token);
}

/// StatusNotifier hosts only offer a left click to items that implement
/// Activate. tray-icon's Linux backend is libappindicator, which exports a menu
/// and nothing else, so a click there can only ever raise the menu. Talking to
/// the host directly is what buys the conventional behaviour: left click opens
/// the window, right click opens the menu.
#[cfg(target_os = "linux")]
mod platform {
    use super::{show_window, tray_image, TrayImage};
    use dioxus::prelude::*;
    use ksni::{menu::StandardItem, Icon, MenuItem, Tray, TrayMethods};
    use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
    use tokio_util::sync::CancellationToken;

    struct OdysseyTray {
        icon: Icon,
        open: UnboundedSender<()>,
        cancellation_token: CancellationToken,
    }

    impl Tray for OdysseyTray {
        fn id(&self) -> String {
            "odyssey-desktop".into()
        }

        fn title(&self) -> String {
            "Odyssey".into()
        }

        fn icon_pixmap(&self) -> Vec<Icon> {
            vec![self.icon.clone()]
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.open.send(());
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            vec![
                StandardItem {
                    label: "Open".into(),
                    activate: Box::new(|tray: &mut Self| {
                        let _ = tray.open.send(());
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Quit".into(),
                    activate: Box::new(|tray: &mut Self| {
                        tray.cancellation_token.cancel();
                        std::process::exit(0);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    /// The wire format is ARGB32 in network byte order, not the RGBA the
    /// decoder hands back.
    fn icon_from(image: TrayImage) -> Icon {
        let mut data = Vec::with_capacity(image.rgba.len());
        for pixel in image.rgba.chunks_exact(4) {
            data.extend_from_slice(&[pixel[3], pixel[0], pixel[1], pixel[2]]);
        }
        Icon {
            width: image.width as i32,
            height: image.height as i32,
            data,
        }
    }

    pub fn use_tray(cancellation_token: CancellationToken) {
        use_future(move || {
            let cancellation_token = cancellation_token.clone();
            async move {
                let (open, mut opens) = unbounded_channel();
                let tray = OdysseyTray {
                    icon: icon_from(tray_image()),
                    open,
                    cancellation_token,
                };

                // The handle has to outlive the loop below: dropping it
                // unregisters the item and the icon disappears.
                let _handle = match tray.spawn().await {
                    Ok(handle) => handle,
                    Err(e) => {
                        dioxus::logger::tracing::error!("failed to register the tray icon: {e}");
                        return;
                    }
                };

                // Activate arrives on the D-Bus task, which cannot touch the
                // window; hop back onto the Dioxus event loop to do that.
                while opens.recv().await.is_some() {
                    show_window();
                }
            }
        });
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::{show_window, tray_image};
    use dioxus::desktop::{
        muda::MenuItem,
        trayicon::{
            menu::Menu, Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
        },
        use_muda_event_handler, use_tray_icon_event_handler,
    };
    use dioxus::prelude::*;

    pub fn use_tray(cancellation_token: tokio_util::sync::CancellationToken) {
        let image = tray_image();
        let icon = Icon::from_rgba(image.rgba, image.width, image.height)
            .expect("failed to build the tray icon");

        let menu = Menu::new();
        let open_item = MenuItem::with_id("open", "Open", true, None);
        let quit_item = MenuItem::with_id("quit", "Quit", true, None);
        if let Err(e) = menu.append_items(&[&open_item, &quit_item]) {
            dioxus::logger::tracing::error!("failed to build the tray menu: {e}");
        }

        // Built by hand rather than through init_tray_icon so the menu above is
        // the one that ships with the icon.
        let builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_icon(icon);

        // Dropping the icon unregisters it, so it lives in the app's context.
        provide_context(builder.build().expect("tray icon builder failed"));

        // Menu clicks surface on the muda channel rather than the tray one, so
        // use_tray_menu_event_handler never sees them.
        use_muda_event_handler(move |event| match event.id.0.as_str() {
            "open" => show_window(),
            "quit" => {
                cancellation_token.cancel();
                std::process::exit(0);
            }
            _ => {}
        });

        use_tray_icon_event_handler(|event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_window();
            }
        });
    }
}
