use dioxus::{
    desktop::{
        muda::MenuItem,
        trayicon::{
            menu::Menu, Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder,
            TrayIconEvent,
        },
        use_muda_event_handler, use_tray_icon_event_handler, window,
    },
    prelude::*,
};

macro_rules! icon_from_bytes {
    ($rel_path:literal) => {{
        let _bytes: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $rel_path));
        let image = image::load_from_memory(_bytes)
            .expect(concat!("Failed to load image from: ", $rel_path))
            .into_rgba8();
        let (icon_width, icon_height) = image.dimensions();
        let icon_rgba = image.into_raw();
        Icon::from_rgba(icon_rgba, icon_width, icon_height)
            .expect(concat!("Failed to build icon from: ", $rel_path))
    }};
}

/// Build the tray icon and wire up its menu.
///
/// Must be called from a component so the hook below runs inside the Dioxus
/// event loop; the main App component is the natural place.
pub fn use_tray_menu(cancellation_token: tokio_util::sync::CancellationToken) -> TrayIcon {
    let icon = icon_from_bytes!("/assets/images/tray-icon.png");

    let menu = Menu::new();

    // muda's PredefinedMenuItem variants have no dbusmenu representation, so a
    // tray built from them exports an empty menu on Linux. Plain items with ids
    // survive the trip and are dispatched below.
    let open_item = MenuItem::with_id("open", "Open", true, None);
    let quit_item = MenuItem::with_id("quit", "Quit", true, None);
    if let Err(e) = menu.append_items(&[&open_item, &quit_item]) {
        dioxus::logger::tracing::error!("failed to build the tray menu: {e}");
    }

    // Built by hand rather than through init_tray_icon so the menu above is the
    // one that ships with the icon. Left click opens the window and right click
    // drops the menu, matching what tray icons do elsewhere.
    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_icon(icon);

    let tray = provide_context(builder.build().expect("tray icon builder failed"));

    // Tray clicks surface on the muda channel rather than the tray one, so
    // use_tray_menu_event_handler never sees them.
    use_muda_event_handler(move |event| match event.id.0.as_str() {
        "open" => {
            show_window();
        }
        "quit" => {
            cancellation_token.cancel();
            std::process::exit(0);
        }
        _ => {}
    });

    use_tray_icon_event_handler(|event| {
        // Ayatana's StatusNotifier hosts deliver a menu request instead of a
        // click, so this fires on Windows and macOS only.
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            show_window();
        }
    });

    tray
}

/// Closing the window only hides it, so the tray is the sole way back.
fn show_window() {
    let window = window();
    window.set_visible(true);
    window.set_focus();
}
