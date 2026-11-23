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

    // Diagnostics that only need USBPcap's unprivileged hub-symlink IOCTL run
    // before the elevation check, so a normal shell can print them.
    #[cfg(target_os = "windows")]
    if args.list_usbpcap {
        resolve::print_usbpcap_diagnostics(args.port.as_deref())?;
        return Ok(());
    }
    #[cfg(not(target_os = "windows"))]
    if args.list_usbpcap {
        bail!("--list-usbpcap is only supported on Windows");
    }

    #[cfg(target_os = "windows")]
    require_admin_for_capture()?;

    let port = match args.port.as_deref() {
        Some(p) => p.to_string(),
        None => auto_detect_port()?,
    };
    let mut info = resolve::resolve(&port)
        .with_context(|| format!("resolving port '{port}'"))?;

    if let Some(addr) = args.usb_address {
        eprintln!(
            "→ Overriding USB device address: {} → {addr}",
            info.devnum
        );
        info.devnum = addr;
    }

    eprintln!(
        "→ Port {}: bus {} device {} VID:PID {:04x}:{:04x}{}",
        port,
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

    // USBPcap exposes one interface per host controller. When the user didn't
    // pick one explicitly, select the interface attached to the device's root
    // hub — the default `\\.\USBPcap1` silently captures nothing when the
    // device hangs off a different controller (common with external hubs).
    #[cfg(target_os = "windows")]
    let auto_usbpcap = if args.usbpcap.is_none() {
        match resolve::auto_usbpcap(&port) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("→ USBPcap auto-detect failed: {e}");
                None
            }
        }
    } else {
        None
    };
    #[cfg(not(target_os = "windows"))]
    let auto_usbpcap: Option<String> = None;

    let usbpcap = args.usbpcap.clone().or(auto_usbpcap);

    #[cfg(target_os = "windows")]
    if usbpcap.is_none() {
        eprintln!("→ Could not determine which USBPcap interface sees this device.");
        eprintln!("  Run with --list-usbpcap to compare root hubs, then pass");
        eprintln!("  --usbpcap '\\\\.\\USBPcapN' explicitly.");
        bail!("USBPcap interface not auto-detected");
    }

    let options = capture::PassiveOptions {
        pcap_path: args.pcap.as_deref(),
        usbpcap_override: usbpcap.as_deref(),
        debug_all: args.debug_usb,
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

#[cfg(target_os = "windows")]
fn require_admin_for_capture() -> Result<()> {
    if elevation::is_elevated()? {
        return Ok(());
    }
    eprintln!("USBPcap requires Administrator privileges to capture.");
    eprintln!();
    eprintln!("Re-run from an elevated shell:");
    eprintln!(r#"  • Right-click PowerShell or Windows Terminal → "Run as administrator","#);
    eprintln!("    then re-run your command, or");
    eprintln!("  • From any PowerShell window:");
    eprintln!("      Start-Process powershell -Verb RunAs");
    eprintln!();
    eprintln!(
        r"(USBPcap's installer sets an Administrators-only ACL on \\.\USBPcapN."
    );
    eprintln!("Capture inherits that, so the binary itself must be elevated.)");
    bail!("not elevated")
}

fn auto_detect_port() -> Result<String> {
    let listed = resolve::list_ports().context("listing USB serial ports")?;
    match listed.len() {
        0 => bail!(
            "no USB serial devices detected. Plug in a device or pass --port."
        ),
        1 => {
            let p = &listed[0];
            eprintln!(
                "→ Auto-detected port: {} (VID:PID {:04x}:{:04x})",
                p.path, p.vid, p.pid
            );
            Ok(p.path.clone())
        }
        _ => {
            let mut msg = String::from(
                "multiple USB serial devices detected; pass --port to choose:\n",
            );
            for p in &listed {
                msg.push_str(&format!(
                    "  {} (VID:PID {:04x}:{:04x})\n",
                    p.path, p.vid, p.pid
                ));
            }
            bail!("{msg}")
        }
    }
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
