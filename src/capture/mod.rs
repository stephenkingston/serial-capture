use std::sync::atomic::{AtomicU64, Ordering};
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

/// Counters shared between the capture loop and the Ctrl-C handler.
///
/// Serial data loss has no in-band signal: the UART, the USB-UART chip and
/// USBPcap's ring buffer all drop silently. Comparing `in_bytes`/`out_bytes`
/// against what the peer application says it sent is the only way to see it,
/// so these totals are printed when capture stops.
#[derive(Default, Debug)]
pub struct Stats {
    /// Packets read from the capture interface (all devices on the bus).
    pub packets: AtomicU64,
    /// Packets whose device address matches the selected port.
    pub device_packets: AtomicU64,
    /// Bulk-IN URBs accepted, and their USB payload bytes.
    pub in_urbs: AtomicU64,
    pub in_bytes: AtomicU64,
    /// Bulk-OUT URBs accepted, and their USB payload bytes.
    pub out_urbs: AtomicU64,
    pub out_bytes: AtomicU64,
    /// Bytes the decoder produced (lower than `*_bytes` when a decoder
    /// strips modem-status bytes).
    pub decoded_in: AtomicU64,
    pub decoded_out: AtomicU64,
}

impl Stats {
    pub fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    pub fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }

    pub fn summary(&self) -> String {
        format!(
            "capture: {} packet(s), {} for this device; \
             IN {} URB(s) / {} B → {} B decoded; \
             OUT {} URB(s) / {} B → {} B decoded",
            Self::get(&self.packets),
            Self::get(&self.device_packets),
            Self::get(&self.in_urbs),
            Self::get(&self.in_bytes),
            Self::get(&self.decoded_in),
            Self::get(&self.out_urbs),
            Self::get(&self.out_bytes),
            Self::get(&self.decoded_out),
        )
    }
}

/// Bridge into the platform-specific capture entry point.
pub struct PassiveOptions<'a> {
    pub pcap_path: Option<&'a Path>,
    /// Windows only: explicit `\\.\USBPcapN` interface override. Ignored on Linux.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub usbpcap_override: Option<&'a str>,
    /// Windows only: capture *every* device on the interface and print a
    /// one-line summary per packet. Diagnostic mode for when the selected
    /// device address shows no traffic. Ignored on Linux.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub debug_all: bool,
    /// Windows only: USBPcap kernel ring-buffer size in bytes. A slow reader
    /// makes the driver drop whole packets silently, so this is the main
    /// insurance against burst loss. Ignored on Linux.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub usbpcap_buffer_bytes: u32,
    /// Capture counters, filled in by the platform backend when present.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub stats: Option<&'a Stats>,
}
