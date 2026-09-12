#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

#[cfg(feature = "desktop")]
fn main() {
    draftnote_core::run();
}

#[cfg(not(feature = "desktop"))]
fn main() {}
