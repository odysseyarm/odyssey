#[cfg(target_os = "windows")]
mod service;

#[cfg(target_os = "windows")]
fn main() {
    service::start();
}

#[cfg(not(target_os = "windows"))]
fn main() {
    println!("not wandows");
}
