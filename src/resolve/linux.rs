use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};

use super::{ListedPort, PortInfo};

pub fn list_ports() -> Result<Vec<ListedPort>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir("/sys/class/tty") {
        Ok(e) => e,
        Err(_) => return Ok(out),
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !(name_str.starts_with("ttyUSB") || name_str.starts_with("ttyACM")) {
            continue;
        }
        let path = format!("/dev/{name_str}");
        if let Ok(info) = resolve(&path) {
            out.push(ListedPort {
                path,
                vid: info.vid,
                pid: info.pid,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

pub fn resolve(port: &str) -> Result<PortInfo> {
    let basename = port
        .strip_prefix("/dev/")
        .unwrap_or(port);

    let device_link = format!("/sys/class/tty/{basename}/device");
    let start = std::fs::canonicalize(&device_link)
        .with_context(|| format!("'{port}' is not a USB-backed serial port (no sysfs entry at {device_link})"))?;

    // ttyACM0 (CDC-ACM): start is the USB interface directly.
    // ttyUSB0 (FTDI/CH340/PL2303): start is a usb-serial-port node *inside* the
    // interface. Walk up to whichever ancestor has bInterfaceNumber.
    let iface_path = find_ancestor_with_file(&start, "bInterfaceNumber", 4)
        .ok_or_else(|| anyhow!("'{port}': could not locate USB interface from {}", start.display()))?;

    let usb_dev = iface_path
        .parent()
        .ok_or_else(|| anyhow!("cannot locate parent USB device of {}", iface_path.display()))?;

    if !usb_dev.join("busnum").exists() {
        bail!("'{port}' is not backed by a USB device");
    }

    let bus: u16 = read_trim(&usb_dev.join("busnum"))?
        .parse()
        .context("parsing busnum")?;
    let devnum: u8 = read_trim(&usb_dev.join("devnum"))?
        .parse()
        .context("parsing devnum")?;
    let vid = u16::from_str_radix(&read_trim(&usb_dev.join("idVendor"))?, 16)
        .context("parsing idVendor")?;
    let pid = u16::from_str_radix(&read_trim(&usb_dev.join("idProduct"))?, 16)
        .context("parsing idProduct")?;

    let interface_number = std::fs::read_to_string(iface_path.join("bInterfaceNumber"))
        .ok()
        .and_then(|s| u8::from_str_radix(s.trim(), 16).ok());

    // Try the chosen interface first; fall back to siblings (CDC-ACM lands on the
    // control interface but the bulk endpoints live on the data interface).
    let mut endpoints = scan_interface_for_bulk(&iface_path);
    if endpoints.bulk_in_ep.is_none() && endpoints.bulk_out_ep.is_none() {
        endpoints = scan_siblings_for_bulk(usb_dev, &iface_path);
    }

    Ok(PortInfo {
        bus,
        devnum,
        vid,
        pid,
        interface_number,
        bulk_in_ep: endpoints.bulk_in_ep,
        bulk_out_ep: endpoints.bulk_out_ep,
        bulk_in_max_packet: endpoints.bulk_in_max_packet,
    })
}

#[derive(Default)]
struct Endpoints {
    bulk_in_ep: Option<u8>,
    bulk_out_ep: Option<u8>,
    bulk_in_max_packet: Option<u16>,
}

fn scan_interface_for_bulk(iface_path: &Path) -> Endpoints {
    let mut out = Endpoints::default();
    let entries = match std::fs::read_dir(iface_path) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("ep_") {
            continue;
        }
        let ep = entry.path();
        let dir = read_trim(&ep.join("direction")).unwrap_or_default();
        let ty = read_trim(&ep.join("type")).unwrap_or_default();
        if ty != "Bulk" {
            continue;
        }
        let addr = read_trim(&ep.join("bEndpointAddress"))
            .ok()
            .and_then(|s| u8::from_str_radix(&s, 16).ok());
        let mps = read_trim(&ep.join("wMaxPacketSize"))
            .ok()
            .and_then(|s| u16::from_str_radix(&s, 16).ok());
        match dir.as_str() {
            "in" if out.bulk_in_ep.is_none() => {
                out.bulk_in_ep = addr;
                out.bulk_in_max_packet = mps;
            }
            "out" if out.bulk_out_ep.is_none() => {
                out.bulk_out_ep = addr;
            }
            _ => {}
        }
    }
    out
}

fn scan_siblings_for_bulk(usb_dev: &Path, skip: &Path) -> Endpoints {
    let usb_dev_name = match usb_dev.file_name().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return Endpoints::default(),
    };
    let prefix = format!("{usb_dev_name}:");
    let entries = match std::fs::read_dir(usb_dev) {
        Ok(e) => e,
        Err(_) => return Endpoints::default(),
    };
    let mut candidates: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p != skip
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(&prefix))
                    .unwrap_or(false)
        })
        .collect();
    candidates.sort();
    for cand in candidates {
        let eps = scan_interface_for_bulk(&cand);
        if eps.bulk_in_ep.is_some() || eps.bulk_out_ep.is_some() {
            return eps;
        }
    }
    Endpoints::default()
}

fn find_ancestor_with_file(start: &Path, name: &str, max_depth: usize) -> Option<PathBuf> {
    let mut current = Some(start);
    for _ in 0..=max_depth {
        let p = current?;
        if p.join(name).exists() {
            return Some(p.to_path_buf());
        }
        current = p.parent();
    }
    None
}

fn read_trim(p: &Path) -> Result<String> {
    Ok(std::fs::read_to_string(p)
        .with_context(|| format!("reading {}", p.display()))?
        .trim()
        .to_string())
}
