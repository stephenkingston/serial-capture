mod capture;
mod cli;
mod decode;
mod elevation;
mod install;
mod output;
mod platform_guard;
mod resolve;

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::capture::Event;
use crate::output::TextSink;

fn main() -> Result<()> {
    platform_guard::check();
    let args = cli::Args::parse();

    if args.active {
        return run_active(args);
    }

    match install::ensure_capture_driver_installed()? {
        install::Outcome::AlreadyInstalled => {}
        install::Outcome::JustInstalled | install::Outcome::ElevatedChildSucceeded => {
            eprintln!("USBPcap installed.");
            eprintln!(
                "→ Unplug and replug your USB device (or reboot), then re-run this command."
            );
            return Ok(());
        }
    }

    let info = resolve::resolve(&args.port)
        .with_context(|| format!("resolving port '{}'", args.port))?;

    eprintln!(
        "→ Port {}: bus {} device {} VID:PID {:04x}:{:04x}{}",
        args.port,
        info.bus,
        info.devnum,
        info.vid,
        info.pid,
        info.interface_number
            .map(|n| format!(" interface {n}"))
            .unwrap_or_default(),
    );

    #[cfg(target_os = "linux")]
    install::linux_usbmon::ensure_ready(info.bus, args.yes)?;

    let decoder = decode::select(
        &info,
        decode::Options {
            ftdi_mps_override: args.ftdi_mps,
        },
    );
    eprintln!(
        "→ Decoder: {}{}",
        decoder.name(),
        match (info.bulk_in_ep, info.bulk_out_ep) {
            (Some(i), Some(o)) => format!(" (bulk IN 0x{i:02x}, OUT 0x{o:02x})"),
            (Some(i), None) => format!(" (bulk IN 0x{i:02x})"),
            (None, Some(o)) => format!(" (bulk OUT 0x{o:02x})"),
            _ => String::new(),
        }
    );
    eprintln!("→ Logging to {}", args.output.display());

    let mut sink = TextSink::create(
        &args.output,
        !args.quiet,
        args.format,
        args.printable_only,
    )?;

    let options = capture::PassiveOptions {
        pcap_path: args.pcap.as_deref(),
    };
    if let Some(p) = options.pcap_path {
        eprintln!("→ Pcapng to {}", p.display());
    }
    eprintln!("→ Press Ctrl-C to stop.");

    let written = Arc::new(AtomicUsize::new(0));
    install_sigint_handler(written.clone(), &args.output, options.pcap_path);

    let on_event = {
        let written = written.clone();
        move |ev: Event| -> Result<()> {
            if sink.write_event(&ev)? {
                written.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        }
    };

    capture::run_passive(info, decoder, options, on_event)
}

#[cfg(target_os = "linux")]
fn run_active(args: cli::Args) -> Result<()> {
    if args.pcap.is_some() {
        bail!(
            "--pcap is not supported in active mode (no USB URB stream to record). \
             Drop --active or drop --pcap."
        );
    }

    eprintln!("→ Active proxy mode (Linux pty bridge).");
    let sink = TextSink::create(
        &args.output,
        !args.quiet,
        args.format,
        args.printable_only,
    )?;
    let sink = std::sync::Arc::new(std::sync::Mutex::new(sink));

    let written = Arc::new(AtomicUsize::new(0));
    install_sigint_handler(written.clone(), &args.output, None);

    capture::run_active(&args.port, args.baud, sink, written)
}

#[cfg(target_os = "windows")]
fn run_active(args: cli::Args) -> Result<()> {
    if args.pcap.is_some() {
        bail!(
            "--pcap is not supported in active mode (no USB URB stream to record). \
             Drop --active or drop --pcap."
        );
    }

    if !install::win_com0com::is_installed() {
        install::win_com0com::print_install_instructions();
        bail!("com0com required for --active");
    }

    eprintln!("→ Active proxy mode (Windows com0com bridge).");
    let sink = TextSink::create(
        &args.output,
        !args.quiet,
        args.format,
        args.printable_only,
    )?;
    let sink = std::sync::Arc::new(std::sync::Mutex::new(sink));

    let written = Arc::new(AtomicUsize::new(0));
    install_sigint_handler(written.clone(), &args.output, None);

    capture::run_active(&args.port, args.baud, sink, written)
}

fn install_sigint_handler(
    written: Arc<AtomicUsize>,
    output: &std::path::Path,
    pcap: Option<&std::path::Path>,
) {
    let output = output.to_owned();
    let pcap = pcap.map(|p| p.to_owned());
    let _ = ctrlc::set_handler(move || {
        let n = written.load(Ordering::Relaxed);
        eprintln!();
        eprintln!("Stopped. Wrote {n} event(s) to {}.", output.display());
        if let Some(p) = pcap.as_ref() {
            eprintln!("Pcapng saved to {}.", p.display());
        }
        std::process::exit(0);
    });
}
