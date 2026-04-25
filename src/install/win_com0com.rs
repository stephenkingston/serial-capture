use std::os::windows::ffi::OsStrExt;
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
};
use windows::core::PCWSTR;

const COM0COM_SERVICE_KEY: &str = r"SYSTEM\CurrentControlSet\Services\com0com";

pub fn is_installed() -> bool {
    let wide: Vec<u16> = std::ffi::OsStr::new(COM0COM_SERVICE_KEY)
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

pub fn print_install_instructions() {
    eprintln!("com0com (virtual serial port driver) is not installed.");
    eprintln!();
    eprintln!("Active proxy mode needs com0com. Auto-install is not yet wired on Windows;");
    eprintln!("for now, install manually:");
    eprintln!();
    eprintln!("  1. Download Pete Batard's signed 2.2.2.0 build:");
    eprintln!("     https://files.akeo.ie/blog/com0com.7z");
    eprintln!("     (or https://sourceforge.net/projects/com0com/files/com0com/2.2.2.0/com0com-2.2.2.0-x64-fre-signed.zip/)");
    eprintln!();
    eprintln!("  2. Extract, then in an Administrator command prompt run:");
    eprintln!("       pnputil /add-driver x64\\com0com.inf /install");
    eprintln!("       setupc.exe install");
    eprintln!();
    eprintln!("  3. Verify Device Manager shows two ports under \"com0com - serial port emulators\"");
    eprintln!("     (default names CNCA0 and CNCB0).");
    eprintln!();
    eprintln!("Then re-run this command. Or use passive mode (drop --active).");
}
