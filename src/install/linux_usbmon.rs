use anyhow::{Context, Result, bail};
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::Command;

/// Make /dev/usbmon{bus} usable by the current user, prompting for sudo if
/// needed. Idempotent: returns immediately when the device is already
/// readable, so the happy path is silent.
pub fn ensure_ready(bus: u16, assume_yes: bool) -> Result<()> {
    let dev = format!("/dev/usbmon{bus}");
    let dev_path = Path::new(&dev);

    if !dev_path.exists() {
        load_module(assume_yes)?;
        if !dev_path.exists() {
            bail!(
                "{dev} still missing after 'sudo modprobe usbmon' \
                 — usbmon is probably not built into your kernel."
            );
        }
    }

    if std::fs::File::open(&dev).is_ok() {
        return Ok(());
    }

    relax_permissions(assume_yes)?;
    if std::fs::File::open(&dev).is_err() {
        bail!(
            "still cannot open {dev} after udev/chmod \
             — check that sudo succeeded and try again."
        );
    }
    Ok(())
}

fn load_module(assume_yes: bool) -> Result<()> {
    eprintln!("→ /dev/usbmon* is missing. The usbmon kernel module is not loaded.");
    if !confirm("Run 'sudo modprobe usbmon'?", assume_yes)? {
        bail!("declined; cannot capture without the usbmon module loaded.");
    }
    sudo(&["modprobe", "usbmon"])
}

fn relax_permissions(assume_yes: bool) -> Result<()> {
    eprintln!(
        "→ /dev/usbmon* is not readable by the current user.\n\
         → Installing a permissive udev rule (mode 0644 / world-readable)\n\
         \x20 at /etc/udev/rules.d/60-serial-capture.rules. Persists across\n\
         \x20 reboots and replugs.\n\
         → NOTE: any local user on this machine will then be able to sniff\n\
         \x20 USB traffic — keystrokes, USB drives, smartcards, anything on\n\
         \x20 the bus. Fine for a single-user dev box; on a shared system,\n\
         \x20 decline and run serial-capture with sudo instead."
    );
    if !confirm(
        "Install udev rule and chmod /dev/usbmon* (one sudo invocation)?",
        assume_yes,
    )? {
        bail!(
            "declined; cannot capture without read access to /dev/usbmon*.\n\
             Re-run with --yes to skip this prompt, or run with sudo."
        );
    }
    // Idempotent: re-running overwrites the rule with the same content and
    // re-chmods, both no-ops on second run.
    let script = r#"set -e
printf 'KERNEL=="usbmon[0-9]*", MODE="0644"\n' > /etc/udev/rules.d/60-serial-capture.rules
udevadm control --reload
chmod 0644 /dev/usbmon*"#;
    sudo(&["sh", "-c", script])
}

fn sudo(args: &[&str]) -> Result<()> {
    let pretty = args.join(" ");
    let status = Command::new("sudo")
        .args(args)
        .status()
        .with_context(|| format!("spawning 'sudo {pretty}'"))?;
    if !status.success() {
        bail!("'sudo {pretty}' exited with {status}");
    }
    Ok(())
}

fn confirm(question: &str, assume_yes: bool) -> Result<bool> {
    if assume_yes {
        eprintln!("{question} [auto-yes]");
        return Ok(true);
    }
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        bail!(
            "{question} — but stdin is not a terminal, so cannot prompt. \
             Re-run with --yes to auto-confirm."
        );
    }
    let mut stderr = std::io::stderr().lock();
    write!(stderr, "{question} [Y/n] ")?;
    stderr.flush()?;
    drop(stderr);

    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    let answer = line.trim().to_lowercase();
    Ok(answer.is_empty() || answer == "y" || answer == "yes")
}
