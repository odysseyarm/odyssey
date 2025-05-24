use dioxus::{desktop::{Config, WindowBuilder}, prelude::*};
use dioxus_router::prelude::*;

#[cfg(windows)]
use windows::{
    core::PCWSTR,
    Win32::Foundation::GetLastError,
    Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, StartServiceW,
        SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS, SERVICE_START,
    },
};

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

use components::Navbar;
use views::Home;

mod components;
mod views;

fn main() {
    #[cfg(windows)]
    {
        if let Err(e) = start_service("OdysseyService") {
            eprintln!("Service start error: {:?}", e);
        }
    }

    dioxus::LaunchBuilder::new()
    .with_cfg(
        Config::default().with_menu(None).with_window(
            WindowBuilder::new()
                .with_maximized(true)
                .with_title("Catchy title")
            )
        )
    .launch(app);
}

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
enum Route {
    #[layout(Navbar)]
    #[route("/")]
    Home {},
}

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");

#[component]
fn app() -> Element {
    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }

        div {
            Router::<Route> {}
        }
    }
}

#[cfg(windows)]
fn to_pcwstr(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn start_service(service_name: &str) -> windows::core::Result<()> {
    use windows::Win32::Foundation::ERROR_SERVICE_ALREADY_RUNNING;

    unsafe {
        let scm = OpenSCManagerW(None, None, SC_MANAGER_CONNECT)?;
        let service_name_w = to_pcwstr(service_name);
        let service = OpenServiceW(
            scm,
            PCWSTR(service_name_w.as_ptr()),
            SERVICE_START | SERVICE_QUERY_STATUS,
        )?;

        let result = StartServiceW(service, Some(&[]));
        if !result.is_ok() {
            let error = GetLastError();
            if error == ERROR_SERVICE_ALREADY_RUNNING {
                println!("Service is already running.");
            } else {
                println!("StartService failed with error: {}", error.0);
            }
        } else {
            println!("Service started successfully.");
        }

        let _ = CloseServiceHandle(service);
        let _ = CloseServiceHandle(scm);
    }

    Ok(())
}
