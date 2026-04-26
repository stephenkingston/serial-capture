use time::OffsetDateTime;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Host → device (application writing to the serial port).
    Out,
    /// Device → host (serial port producing data for the application).
    In,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub ts: OffsetDateTime,
    pub dir: Direction,
    pub bytes: Vec<u8>,
}

use std::path::Path;

#[cfg(target_os = "linux")]
mod passive_linux;
#[cfg(target_os = "linux")]
pub use passive_linux::run_passive;

#[cfg(target_os = "windows")]
mod passive_win;
#[cfg(target_os = "windows")]
pub use passive_win::run_passive;

/// Bridge into the platform-specific capture entry point.
pub struct PassiveOptions<'a> {
    pub pcap_path: Option<&'a Path>,
}
