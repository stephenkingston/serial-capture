mod capture;
mod cli;
mod decode;
mod elevation;
mod install;
mod output;
mod platform_guard;
mod resolve;

use anyhow::{Context, Result};
use clap::Parser;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::capture::Event;
use crate::output::TextSink;

fn main() -> Result<()> {
    platform_guard::check();
    let args = cli::Args::parse();

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
    eprintln!("→ Logging to {}", describe_dest(args.output.as_deref(), args.quiet));

    let mut sink = TextSink::create(
        args.output.as_deref(),
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
    install_sigint_handler(written.clone(), args.output.as_deref(), options.pcap_path);

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

fn describe_dest(output: Option<&std::path::Path>, quiet: bool) -> String {
    match (output, quiet) {
        (Some(p), _) => p.display().to_string(),
        (None, false) => "stdout".to_string(),
        (None, true) => "(discarded — --quiet without --output)".to_string(),
    }
}

fn install_sigint_handler(
    written: Arc<AtomicUsize>,
    output: Option<&std::path::Path>,
    pcap: Option<&std::path::Path>,
) {
    let output = output.map(|p| p.to_owned());
    let pcap = pcap.map(|p| p.to_owned());
    let _ = ctrlc::set_handler(move || {
        let n = written.load(Ordering::Relaxed);
        eprintln!();
        match output.as_ref() {
            Some(p) => eprintln!("Stopped. Wrote {n} event(s) to {}.", p.display()),
            None => eprintln!("Stopped. Wrote {n} event(s)."),
        }
        if let Some(p) = pcap.as_ref() {
            eprintln!("Pcapng saved to {}.", p.display());
        }
        std::process::exit(0);
    });
}
