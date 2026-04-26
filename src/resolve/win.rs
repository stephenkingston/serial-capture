use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD, REG_SZ, RegCloseKey, RegEnumKeyExW,
    RegOpenKeyExW, RegQueryValueExW,
};
use windows::core::PCWSTR;

use super::{ListedPort, PortInfo};

const USB_ENUM_KEY: &str = r"SYSTEM\CurrentControlSet\Enum\USB";

pub fn list_ports() -> Result<Vec<ListedPort>> {
    let mut out: Vec<ListedPort> = Vec::new();
    let usb_root = open_subkey(HKEY_LOCAL_MACHINE, USB_ENUM_KEY)
        .with_context(|| format!("opening {USB_ENUM_KEY}"))?;
    let walk = (|| -> Result<()> {
        for vid_pid_name in enum_subkeys(usb_root)? {
            let parsed = match parse_vidpid(&vid_pid_name) {
                Some(t) => t,
                None => continue,
            };
            let vp_handle = match open_subkey(usb_root, &vid_pid_name) {
                Ok(h) => h,
                Err(_) => continue,
            };
            let _: Result<()> = (|| -> Result<()> {
                for instance_name in enum_subkeys(vp_handle)? {
                    let inst_handle = match open_subkey(vp_handle, &instance_name) {
                        Ok(h) => h,
                        Err(_) => continue,
                    };
                    let port_name = read_string(inst_handle, "Device Parameters", "PortName");
                    unsafe { let _ = RegCloseKey(inst_handle); }
                    if let Some(name) = port_name {
                        let trimmed = name.trim();
                        if trimmed.to_ascii_uppercase().starts_with("COM") {
                            out.push(ListedPort {
                                path: trimmed.to_string(),
                                vid: parsed.0,
                                pid: parsed.1,
                            });
                        }
                    }
                }
                Ok(())
            })();
            unsafe { let _ = RegCloseKey(vp_handle); }
        }
        Ok(())
    })();
    unsafe { let _ = RegCloseKey(usb_root); }
    walk?;
    // Stable order, deduplicate composite-interface duplicates that report the
    // same PortName from multiple interface keys.
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    Ok(out)
}

pub fn resolve(port: &str) -> Result<PortInfo> {
    let want = port.trim().to_ascii_uppercase();
    if !want.starts_with("COM") {
        bail!("expected port like 'COM4', got '{port}'");
    }

    let dev = find_usb_device_for_com(&want)
        .with_context(|| format!("looking up '{want}' under {USB_ENUM_KEY}"))?
        .ok_or_else(|| anyhow::anyhow!("no USB device exposes {want}"))?;

    Ok(PortInfo {
        bus: 0, // not directly available from the registry; USBPcap reports bus per-packet
        devnum: dev.address,
        vid: dev.vid,
        pid: dev.pid,
        interface_number: dev.interface_number,
        // Endpoint discovery on Windows requires either WinUSB or descriptor parsing
        // via SetupAPI; deferred to a later milestone. The decoder will default
        // bulk_in_max_packet to 64 (correct for FT232/CH340/PL2303 full-speed).
        bulk_in_ep: None,
        bulk_out_ep: None,
        bulk_in_max_packet: None,
    })
}

struct Found {
    vid: u16,
    pid: u16,
    address: u8,
    interface_number: Option<u8>,
}

fn find_usb_device_for_com(com: &str) -> Result<Option<Found>> {
    let usb_root = open_subkey(HKEY_LOCAL_MACHINE, USB_ENUM_KEY)?;
    let result = (|| -> Result<Option<Found>> {
        for vid_pid_name in enum_subkeys(usb_root)? {
            let parsed = match parse_vidpid(&vid_pid_name) {
                Some(t) => t,
                None => continue,
            };
            let vp_handle = match open_subkey(usb_root, &vid_pid_name) {
                Ok(h) => h,
                Err(_) => continue,
            };
            let result = (|| -> Result<Option<Found>> {
                for instance_name in enum_subkeys(vp_handle)? {
                    let inst_handle = match open_subkey(vp_handle, &instance_name) {
                        Ok(h) => h,
                        Err(_) => continue,
                    };
                    let port_name = read_string(inst_handle, "Device Parameters", "PortName");
                    let matches = port_name
                        .as_deref()
                        .map(|s| s.trim().eq_ignore_ascii_case(com))
                        .unwrap_or(false);
                    if !matches {
                        unsafe { let _ = RegCloseKey(inst_handle); }
                        continue;
                    }
                    let address = read_dword(inst_handle, "", "Address").unwrap_or(0) as u8;
                    unsafe { let _ = RegCloseKey(inst_handle); }
                    return Ok(Some(Found {
                        vid: parsed.0,
                        pid: parsed.1,
                        address,
                        interface_number: parsed.2,
                    }));
                }
                Ok(None)
            })();
            unsafe { let _ = RegCloseKey(vp_handle); }
            if let Some(found) = result? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    })();
    unsafe { let _ = RegCloseKey(usb_root); }
    result
}

fn parse_vidpid(s: &str) -> Option<(u16, u16, Option<u8>)> {
    let mut vid: Option<u16> = None;
    let mut pid: Option<u16> = None;
    let mut mi: Option<u8> = None;
    for part in s.split('&') {
        if let Some(v) = part.strip_prefix("VID_") {
            vid = u16::from_str_radix(v, 16).ok();
        } else if let Some(p) = part.strip_prefix("PID_") {
            pid = u16::from_str_radix(p, 16).ok();
        } else if let Some(m) = part.strip_prefix("MI_") {
            mi = u8::from_str_radix(m, 16).ok();
        }
    }
    Some((vid?, pid?, mi))
}

fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn open_subkey(parent: HKEY, path: &str) -> Result<HKEY> {
    let wide = to_wide(path);
    let mut handle = HKEY::default();
    let rc = unsafe {
        RegOpenKeyExW(parent, PCWSTR(wide.as_ptr()), 0, KEY_READ, &mut handle)
    };
    win_err(rc, || format!("RegOpenKeyExW({path})"))?;
    Ok(handle)
}

fn enum_subkeys(handle: HKEY) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut idx = 0u32;
    loop {
        let mut name = [0u16; 256];
        let mut name_len = name.len() as u32;
        let rc = unsafe {
            RegEnumKeyExW(
                handle,
                idx,
                windows::core::PWSTR(name.as_mut_ptr()),
                &mut name_len,
                None,
                windows::core::PWSTR::null(),
                None,
                None,
            )
        };
        if rc == ERROR_NO_MORE_ITEMS {
            break;
        }
        win_err(rc, || format!("RegEnumKeyExW(idx={idx})"))?;
        out.push(OsString::from_wide(&name[..name_len as usize])
            .to_string_lossy()
            .into_owned());
        idx += 1;
    }
    Ok(out)
}

fn read_string(parent: HKEY, subpath: &str, value: &str) -> Option<String> {
    let handle = if subpath.is_empty() {
        parent
    } else {
        match open_subkey(parent, subpath) {
            Ok(h) => h,
            Err(_) => return None,
        }
    };
    let result = read_string_value(handle, value);
    if !subpath.is_empty() {
        unsafe { let _ = RegCloseKey(handle); }
    }
    result
}

fn read_string_value(handle: HKEY, value: &str) -> Option<String> {
    let wide = to_wide(value);
    let mut kind = REG_SZ;
    let mut len = 0u32;
    let rc = unsafe {
        RegQueryValueExW(
            handle,
            PCWSTR(wide.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut len),
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    let rc = unsafe {
        RegQueryValueExW(
            handle,
            PCWSTR(wide.as_ptr()),
            None,
            Some(&mut kind),
            Some(buf.as_mut_ptr()),
            Some(&mut len),
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let wbuf: &[u16] = unsafe {
        std::slice::from_raw_parts(buf.as_ptr() as *const u16, buf.len() / 2)
    };
    let trimmed: &[u16] = match wbuf.iter().position(|&c| c == 0) {
        Some(i) => &wbuf[..i],
        None => wbuf,
    };
    Some(OsString::from_wide(trimmed).to_string_lossy().into_owned())
}

fn read_dword(parent: HKEY, subpath: &str, value: &str) -> Option<u32> {
    let handle = if subpath.is_empty() {
        parent
    } else {
        match open_subkey(parent, subpath) {
            Ok(h) => h,
            Err(_) => return None,
        }
    };
    let wide = to_wide(value);
    let mut kind = REG_DWORD;
    let mut data = 0u32;
    let mut len = 4u32;
    let rc = unsafe {
        RegQueryValueExW(
            handle,
            PCWSTR(wide.as_ptr()),
            None,
            Some(&mut kind),
            Some(&mut data as *mut _ as *mut u8),
            Some(&mut len),
        )
    };
    if !subpath.is_empty() {
        unsafe { let _ = RegCloseKey(handle); }
    }
    if rc == ERROR_SUCCESS { Some(data) } else { None }
}

fn win_err(rc: WIN32_ERROR, ctx: impl FnOnce() -> String) -> Result<()> {
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        bail!("{}: WIN32 error {}", ctx(), rc.0)
    }
}
