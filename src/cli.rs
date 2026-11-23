use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "serial-capture",
    about = "Capture serial traffic on a USB virtual COM port (CH340, FT232, FT2232, PL2303, CP210x, CDC-ACM).",
    version,
)]
pub struct Args {
    /// Serial port to capture (e.g. COM4 on Windows, /dev/ttyUSB0 or /dev/ttyACM0
    /// on Linux). If omitted, the tool auto-detects when exactly one USB serial
    /// device is connected; otherwise it lists candidates and exits.
    #[arg(long, value_name = "PORT")]
    pub port: Option<String>,

    /// Output text file path. If omitted, decoded events are written to
    /// stdout instead. Combine with --quiet to suppress text output entirely
    /// (useful with --pcap).
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Also write a Wireshark-compatible pcapng capture.
    #[arg(long, value_name = "FILE")]
    pub pcap: Option<PathBuf>,

    /// In the ASCII column, replace non-printable bytes with '.'.
    #[arg(long)]
    pub printable_only: bool,

    /// Suppress live tail to stdout.
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Print one row per captured URB instead of reassembling lines. By
    /// default bytes from consecutive URBs are joined and split on newlines,
    /// because some drivers (PL2303 on Windows) deliver one byte at a time.
    #[arg(long)]
    pub no_assemble: bool,

    /// Auto-confirm setup steps that need sudo (Linux: load usbmon, relax
    /// /dev/usbmon* permissions). Without this, you're prompted before each
    /// sudo invocation. Has no effect when setup is already complete.
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Output format for the text log.
    #[arg(long, value_enum, default_value_t = Format::Both)]
    pub format: Format,

    /// Force FTDI bulk-IN wMaxPacketSize. Use 64 for FT232/FT-X (full-speed)
    /// or 512 for FT2232H/FT4232H/FT232H (high-speed). Only needed when
    /// auto-detection picks the wrong size for an unusual variant or clone.
    #[arg(long, value_name = "BYTES")]
    pub ftdi_mps: Option<u16>,

    /// Windows only: pick a specific USBPcap interface (e.g.
    /// `\\.\USBPcap2`). USBPcap installs one filter per host controller; a
    /// device on a controller other than the first won't show traffic on
    /// `\\.\USBPcap1`. Defaults to matching the device's root hub, and fails
    /// with instructions if no interface matches.
    #[arg(long, value_name = "NAME")]
    pub usbpcap: Option<String>,

    /// Windows only: print the root hub of each USBPcap interface and of the
    /// selected port, then exit. Use this when auto-detection fails.
    #[arg(long)]
    pub list_usbpcap: bool,

    /// Windows only: capture every device on the selected USBPcap interface and
    /// print a one-line summary per packet (`dev`, endpoint, transfer type,
    /// length, first bytes). Use this to find the real device address when the
    /// port shows no traffic.
    #[arg(long)]
    pub debug_usb: bool,

    /// Windows only: override the USB device address used for filtering. Use
    /// when the address reported by Windows is stale (the probe output shows
    /// the real one).
    #[arg(long, value_name = "N")]
    pub usb_address: Option<u8>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum Format {
    /// Hex bytes only.
    Hex,
    /// ASCII column only.
    Ascii,
    /// Both columns (default).
    Both,
}
