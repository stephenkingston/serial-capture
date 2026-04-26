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
            "still cannot open {dev} after chmod \
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
         → For convenience this run will chmod the device files 0644 so any\n\
         \x20 user on this machine can read USB traffic. For a tighter, persistent\n\
         \x20 setup, install a udev rule + group instead (see README)."
    );
    if !confirm("Run 'sudo chmod 0644 /dev/usbmon*'?", assume_yes)? {
        bail!(
            "declined; cannot capture without read access to /dev/usbmon*.\n\
             Re-run with --yes to skip this prompt, or set up the udev rule manually."
        );
    }
    sudo(&["sh", "-c", "chmod 0644 /dev/usbmon*"])
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
