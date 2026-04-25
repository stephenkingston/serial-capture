use anyhow::{Context, Result, anyhow, bail};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
};
use windows::core::PCWSTR;

/// USBPcap installer (NSIS), embedded at build time by build.rs.
const INSTALLER_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/USBPcapSetup.exe"));

const USBPCAP_SERVICE_KEY: &str = r"SYSTEM\CurrentControlSet\Services\USBPcap";

pub fn is_installed() -> bool {
    let wide: Vec<u16> = std::ffi::OsStr::new(USBPCAP_SERVICE_KEY)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut handle = HKEY::default();
    let rc = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(wide.as_ptr()),
            0,
            KEY_READ,
            &mut handle,
        )
    };
    if rc.is_ok() {
        unsafe { let _ = RegCloseKey(handle); }
        true
    } else {
        false
    }
}

/// Extract the embedded installer to a temp directory, run it silently, and
/// clean up. Returns Ok(()) on installer exit code 0; Err otherwise.
pub fn install_silent() -> Result<()> {
    let mut temp = std::env::temp_dir();
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    temp.push(format!("serial-capture-install-{token}"));
    std::fs::create_dir_all(&temp)
        .with_context(|| format!("creating temp dir {}", temp.display()))?;

    let installer_path: PathBuf = temp.join("USBPcapSetup.exe");
    std::fs::write(&installer_path, INSTALLER_BYTES)
        .with_context(|| format!("writing installer to {}", installer_path.display()))?;

    eprintln!("Installing USBPcap 1.5.4.0 (silent)...");
    let status = Command::new(&installer_path)
        .arg("/S")
        .status()
        .with_context(|| format!("spawning {}", installer_path.display()))?;

    let _ = std::fs::remove_file(&installer_path);
    let _ = std::fs::remove_dir(&temp);

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Err(anyhow!("USBPcap installer exited with code {code}"));
    }

    if !is_installed() {
        bail!(
            "USBPcap installer reported success but the service key is not present. \
             You may need to reboot for the driver to register."
        );
    }
    Ok(())
}
