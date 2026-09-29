pub mod app;
pub mod config;
pub mod conflict;
pub mod local;
pub mod note;
pub mod secret;
pub mod sync;
pub mod syncclient;

#[cfg(any(feature = "desktop", target_os = "ios"))]
pub mod commands;

#[cfg(any(feature = "desktop", target_os = "ios"))]
pub use commands::run;
