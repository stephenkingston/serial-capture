#[cfg(target_os = "windows")]
mod win;

#[cfg(target_os = "windows")]
pub use win::{is_elevated, relaunch_self_elevated_and_wait};
