use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "serial-capture",
    about = "Capture serial traffic on a USB virtual COM port (CH340, FT232, FT2232, PL2303, CDC-ACM).",
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
