//! Windows port resolution.
//!
//! Enumerates *present* ports through SetupAPI, so unplugged devices that
//! linger in the registry (the classic "ghost COM port") are ignored. From a
//! present port we walk the device tree up to its USB parent. The walk is what
//! makes FTDI ports work: FTDI's driver enumerates them under `Enum\FTDIBUS`,
//! not `Enum\USB`. Composite CDC-ACM ports (whose instance ID carries `&MI_`)
//! resolve to the physical USB device the same way.

use anyhow::{Context, Result, anyhow, bail};
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_ID_Size, CM_Get_Device_IDW, CM_Get_Parent, CM_LOCATE_DEVNODE_NORMAL,
    CM_Locate_DevNodeW, DICS_FLAG_GLOBAL, DIGCF_PRESENT, DIREG_DEV, GUID_DEVCLASS_PORTS,
    HDEVINFO, SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo,
    SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW, SetupDiOpenDevRegKey,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HWND, WIN32_ERROR};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD, REG_SZ, RegCloseKey, RegOpenKeyExW,
    RegQueryValueExW,
};
use windows::core::PCWSTR;

use super::{ListedPort, PortInfo};

/// `CTL_CODE(FILE_DEVICE_UNKNOWN, 0x803, METHOD_BUFFERED, FILE_ANY_ACCESS)`
const IOCTL_USBPCAP_GET_HUB_SYMLINK: u32 = 0x0022_200C;

pub fn list_ports() -> Result<Vec<ListedPort>> {
    let mut out = Vec::new();
    for port in present_ports()? {
        let Some((_, vid, pid, _)) = port_to_usb(&port) else {
            continue;
        };
        out.push(ListedPort {
            path: port.com,
            vid,
            pid,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    Ok(out)
}

pub fn resolve(port: &str) -> Result<PortInfo> {
    let want = port.trim();
    if !want.to_ascii_uppercase().starts_with("COM") {
        bail!("expected port like 'COM4', got '{port}'");
    }

    let ports = present_ports().context("enumerating present serial ports")?;
    let found = ports
        .iter()
        .find(|p| p.com.eq_ignore_ascii_case(want))
        .ok_or_else(|| {
            if ports.is_empty() {
                anyhow!("no present serial port named '{want}' (none are currently present)")
            } else {
                let names = ports
                    .iter()
                    .map(|p| p.com.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow!("no present serial port named '{want}'; present ports: {names}")
            }
        })?;

    let (usb_id, vid, pid, interface_number) = port_to_usb(found)
        .ok_or_else(|| anyhow!("'{want}' is not backed by a USB device"))?;
    let devnum = usb_address(&usb_id);
    if devnum == 0 {
        bail!("could not read the USB device address for '{want}'; replug the device and retry");
    }

    Ok(PortInfo {
        bus: 0, // not needed on Windows; USBPcap reports the bus per packet
        devnum,
        vid,
        pid,
        interface_number,
        // Endpoint discovery on Windows requires either WinUSB or descriptor
        // parsing via SetupAPI; deferred. Decoders default to 64 bytes, which
        // is correct for the full-speed chips we support.
        bulk_in_ep: None,
        bulk_out_ep: None,
        bulk_in_max_packet: None,
    })
}

/// Pick the `\\.\USBPcapN` interface whose root hub is the one this port is
/// attached to. USBPcap exposes one interface per host controller; using the
/// wrong one silently captures nothing.
pub fn auto_usbpcap(port: &str) -> Result<Option<String>> {
    let want = port.trim();
    let ports = present_ports()?;
    let Some(found) = ports.iter().find(|p| p.com.eq_ignore_ascii_case(want)) else {
        return Ok(None);
    };
    let Some(root) = root_hub_instance(&found.instance_id) else {
        return Ok(None);
    };
    let root = normalize_device_path(&root);

    for n in 1..=32u32 {
        let name = format!(r"\\.\USBPcap{n}");
        if let Some(symlink) = usbpcap_hub_symlink(&name)
            && normalize_device_path(&symlink) == root
        {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

struct PresentPort {
    com: String,
    /// Device instance ID, e.g. `USB\VID_067B&PID_2303\5&...` or
    /// `FTDIBUS\VID_0403+PID_6001+A50285BI\0000`.
    instance_id: String,
}

fn port_to_usb(port: &PresentPort) -> Option<(String, u16, u16, Option<u8>)> {
    let usb_id = usb_parent_instance(&port.instance_id)?;
    let (vid, pid, _) = parse_vidpid(&usb_id)?;
    Some((usb_id, vid, pid, parse_interface(&port.instance_id)))
}

/// Enumerate ports that are present *right now* (DIGCF_PRESENT), and read each
/// one's `PortName` from its driver key.
fn present_ports() -> Result<Vec<PresentPort>> {
    let mut out = Vec::new();
    unsafe {
        let hdev = SetupDiGetClassDevsW(
            Some(&GUID_DEVCLASS_PORTS),
            PCWSTR::null(),
            HWND::default(),
            DIGCF_PRESENT,
        )
        .context("SetupDiGetClassDevsW(Ports)")?;
        let _guard = DevInfoGuard(hdev);

        let mut index = 0u32;
        loop {
            let mut data = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInfo(hdev, index, &mut data).is_err() {
                break;
            }
            index += 1;

            let Some(instance_id) = device_instance_id(hdev, &data) else {
                continue;
            };
            let Ok(hkey) =
                SetupDiOpenDevRegKey(hdev, &data, DICS_FLAG_GLOBAL.0, 0, DIREG_DEV, KEY_READ.0)
            else {
                continue;
            };
            let port_name = read_string_value(hkey, "PortName");
            let _ = RegCloseKey(hkey);
            let Some(name) = port_name else { continue };
            let com = name.trim().to_string();
            if com.to_ascii_uppercase().starts_with("COM") {
                out.push(PresentPort { com, instance_id });
            }
        }
    }
    Ok(out)
}

/// Walk parents until we reach the physical USB device. Composite interfaces
/// (`...&MI_00`) are skipped in favour of their parent; FTDI ports start at an
/// `FTDIBUS\...` node and end at the `USB\VID_...&PID_...` node.
fn usb_parent_instance(instance_id: &str) -> Option<String> {
    let mut devinst = 0u32;
    let wide = to_wide(instance_id);
    let rc = unsafe {
        CM_Locate_DevNodeW(
            &mut devinst,
            PCWSTR(wide.as_ptr()),
            CM_LOCATE_DEVNODE_NORMAL,
        )
    };
    if rc.0 != 0 {
        return None;
    }

    for _ in 0..16 {
        let id = device_id(devinst)?;
        if id.starts_with("USB\\") && !id.contains("&MI_") {
            return Some(id);
        }
        let mut parent = 0u32;
        let rc = unsafe { CM_Get_Parent(&mut parent, devinst, 0) };
        if rc.0 != 0 {
            return None;
        }
        devinst = parent;
    }
    None
}

fn root_hub_instance(instance_id: &str) -> Option<String> {
    let mut devinst = 0u32;
    let wide = to_wide(instance_id);
    let rc = unsafe {
        CM_Locate_DevNodeW(
            &mut devinst,
            PCWSTR(wide.as_ptr()),
            CM_LOCATE_DEVNODE_NORMAL,
        )
    };
    if rc.0 != 0 {
        return None;
    }

    let mut last = device_id(devinst)?;
    for _ in 0..16 {
        let mut parent = 0u32;
        let rc = unsafe { CM_Get_Parent(&mut parent, devinst, 0) };
        if rc.0 != 0 {
            break;
        }
        devinst = parent;
        if let Some(id) = device_id(devinst) {
            last = id;
        }
    }
    Some(last)
}

fn device_id(devinst: u32) -> Option<String> {
    let mut len = 0u32;
    let rc = unsafe { CM_Get_Device_ID_Size(&mut len, devinst, 0) };
    if rc.0 != 0 || len == 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize + 1];
    let rc = unsafe { CM_Get_Device_IDW(devinst, &mut buf, 0) };
    if rc.0 != 0 {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

fn device_instance_id(hdev: HDEVINFO, data: &SP_DEVINFO_DATA) -> Option<String> {
    let mut required = 0u32;
    let _ = unsafe { SetupDiGetDeviceInstanceIdW(hdev, data, None, Some(&mut required)) };
    if required == 0 {
        return None;
    }
    let mut buf = vec![0u16; required as usize];
    unsafe { SetupDiGetDeviceInstanceIdW(hdev, data, Some(&mut buf), None).ok()? };
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

fn usb_address(usb_instance_id: &str) -> u8 {
    let path = format!(r"SYSTEM\CurrentControlSet\Enum\{usb_instance_id}");
    match open_subkey(HKEY_LOCAL_MACHINE, &path) {
        Ok(handle) => {
            let address = read_dword(handle, "", "Address").unwrap_or(0) as u8;
            unsafe { let _ = RegCloseKey(handle); }
            address
        }
        Err(_) => 0,
    }
}

/// Normalize both CfgMgr instance IDs (`USB\ROOT_HUB30\4&...`) and USBPcap's
/// hub symlinks (`\\?\USB#ROOT_HUB30#4&...#{guid}`) to a comparable form.
fn normalize_device_path(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix(r"\\?\")
        .or_else(|| s.strip_prefix(r"\??\"))
        .unwrap_or(s);
    let s = s.split("#{").next().unwrap_or(s);
    s.replace('#', "\\").to_ascii_lowercase()
}

fn usbpcap_hub_symlink(path: &str) -> Option<String> {
    let wide = to_wide(path);
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .ok()?;

    let mut buf = vec![0u16; 1024];
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_USBPCAP_GET_HUB_SYMLINK,
            None,
            0,
            Some(buf.as_mut_ptr() as *mut _),
            (buf.len() * std::mem::size_of::<u16>()) as u32,
            Some(&mut returned),
            None,
        )
    }
    .is_ok();
    unsafe { let _ = CloseHandle(handle); }
    if !ok || returned == 0 {
        return None;
    }

    let chars = (returned as usize / 2).min(buf.len());
    let n = buf[..chars].iter().position(|&c| c == 0).unwrap_or(chars);
    Some(String::from_utf16_lossy(&buf[..n]))
}

struct DevInfoGuard(HDEVINFO);
impl Drop for DevInfoGuard {
    fn drop(&mut self) {
        unsafe { let _ = SetupDiDestroyDeviceInfoList(self.0); }
    }
}

fn parse_interface(instance_id: &str) -> Option<u8> {
    instance_id
        .split(|c| c == '&' || c == '\\')
        .find_map(|part| part.strip_prefix("MI_"))
        .and_then(|s| u8::from_str_radix(s, 16).ok())
}

fn parse_vidpid(s: &str) -> Option<(u16, u16, Option<u8>)> {
    let mut vid: Option<u16> = None;
    let mut pid: Option<u16> = None;
    let mut mi: Option<u8> = None;
    for part in s.split(|c| c == '&' || c == '\\' || c == '+') {
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
    let wbuf: &[u16] =
        unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u16, buf.len() / 2) };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_cfgmgr_and_usbpcap_paths() {
        let root = r"USB\ROOT_HUB30\4&2e5d1f&0&0";
        let symlink =
            r"\\?\USB#ROOT_HUB30#4&2e5d1f&0&0#{f18a0e88-c30c-11d0-8815-00a0c906bed8}";
        assert_eq!(
            normalize_device_path(root),
            normalize_device_path(symlink)
        );
    }

    #[test]
    fn parses_composite_interface() {
        assert_eq!(
            parse_interface(r"USB\VID_2341&PID_0043&MI_00\6&1a2b3c&0&0000"),
            Some(0)
        );
        assert_eq!(parse_interface(r"USB\VID_067B&PID_2303\5&2a3b"), None);
    }

    #[test]
    fn parses_vid_pid_from_usb_instance() {
        assert_eq!(
            parse_vidpid(r"USB\VID_10C4&PID_EA60\0001"),
            Some((0x10c4, 0xea60, None))
        );
        assert_eq!(
            parse_vidpid(r"USB\VID_2341&PID_0043&MI_00\6&1"),
            Some((0x2341, 0x0043, Some(0)))
        );
    }
}
