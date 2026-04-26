use anyhow::Result;

#[cfg(target_os = "linux")]
pub mod linux_usbmon;

#[cfg(target_os = "windows")]
mod win_usbpcap;

#[derive(Debug)]
#[allow(dead_code)] // Some variants are only constructed on Windows.
pub enum Outcome {
    /// Capture driver was already installed on entry. Proceed normally.
    AlreadyInstalled,
    /// We just installed it (either as the elevated child or because we were
    /// already elevated). Caller should print a replug message and exit.
    JustInstalled,
    /// We re-launched ourselves elevated and the child reports success. The
    /// caller should print a replug message and exit.
    ElevatedChildSucceeded,
}

#[cfg(target_os = "linux")]
pub fn ensure_capture_driver_installed() -> Result<Outcome> {
    // On Linux, capture is via usbmon (in-tree kernel module). Detection is
    // handled by the capture preflight in capture::passive_linux to keep
    // failure context near the operation. Nothing to install here.
    Ok(Outcome::AlreadyInstalled)
}

#[cfg(target_os = "windows")]
pub fn ensure_capture_driver_installed() -> Result<Outcome> {
    if win_usbpcap::is_installed() {
        return Ok(Outcome::AlreadyInstalled);
    }

    let elevated = crate::elevation::is_elevated()?;
    if elevated {
        win_usbpcap::install_silent()?;
        return Ok(Outcome::JustInstalled);
    }

    eprintln!(
        "USBPcap is not installed. A UAC prompt will appear so we can install it silently."
    );
    let exit_code = crate::elevation::relaunch_self_elevated_and_wait()?;
    if exit_code == 0 {
        Ok(Outcome::ElevatedChildSucceeded)
    } else {
        anyhow::bail!(
            "elevated USBPcap installer exited with code {exit_code} \
             — re-run as Administrator to see the failure detail."
        )
    }
}
