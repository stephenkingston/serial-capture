use anyhow::{Context, Result, anyhow, bail};
use std::os::windows::ffi::OsStrExt;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0};
use windows::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::PCWSTR;

pub fn is_elevated() -> Result<bool> {
    let mut token = HANDLE::default();
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| anyhow!("OpenProcessToken: {e}"))?;
    }
    let mut elev = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut size = 0u32;
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elev as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        )
    };
    unsafe { let _ = CloseHandle(token); }
    result.map_err(|e| anyhow!("GetTokenInformation: {e}"))?;
    Ok(elev.TokenIsElevated != 0)
}

/// Re-launch our own executable with the current arguments, elevated via UAC.
/// Blocks until the elevated child exits and returns its exit code.
pub fn relaunch_self_elevated_and_wait() -> Result<i32> {
    let exe = std::env::current_exe().context("getting current_exe()")?;
    let exe_w: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let args = build_arg_string();
    let args_w: Vec<u16> = std::ffi::OsStr::new(&args)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let verb_w: Vec<u16> = std::ffi::OsStr::new("runas")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb_w.as_ptr()),
        lpFile: PCWSTR(exe_w.as_ptr()),
        lpParameters: PCWSTR(args_w.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };

    unsafe { ShellExecuteExW(&mut info) }
        .map_err(|e| anyhow!("ShellExecuteExW (UAC): {e} — did you decline the prompt?"))?;

    if info.hProcess.is_invalid() {
        bail!("ShellExecuteExW returned no process handle");
    }

    let wait = unsafe { WaitForSingleObject(info.hProcess, INFINITE) };
    if wait == WAIT_FAILED {
        unsafe { let _ = CloseHandle(info.hProcess); }
        bail!("WaitForSingleObject failed waiting for elevated child");
    }
    if wait != WAIT_OBJECT_0 {
        unsafe { let _ = CloseHandle(info.hProcess); }
        bail!("unexpected wait result {wait:?} on elevated child");
    }

    let mut exit_code: u32 = 0;
    unsafe {
        GetExitCodeProcess(info.hProcess, &mut exit_code)
            .map_err(|e| anyhow!("GetExitCodeProcess: {e}"))?;
        let _ = CloseHandle(info.hProcess);
    }
    Ok(exit_code as i32)
}

fn build_arg_string() -> String {
    std::env::args()
        .skip(1)
        .map(quote_if_needed)
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_if_needed(arg: String) -> String {
    if arg.is_empty() || arg.contains(' ') || arg.contains('"') || arg.contains('\t') {
        let escaped = arg.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        arg
    }
}
