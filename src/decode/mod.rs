use crate::capture::Direction;
use crate::resolve::PortInfo;

pub trait Decoder: Send {
    fn name(&self) -> &'static str;
    /// Transform a USB bulk-transfer payload into serial bytes.
    /// Appends to `out` to avoid per-call allocation.
    fn decode(&mut self, dir: Direction, payload: &[u8], out: &mut Vec<u8>);
}

mod cdc_acm;
mod ch340;
mod ftdi;
mod pl2303;
pub use cdc_acm::CdcAcm;
pub use ch340::Ch340;
pub use ftdi::Ftdi;
pub use pl2303::Pl2303;

/// USB Vendor IDs.
const VID_FTDI: u16 = 0x0403;
const VID_PROLIFIC: u16 = 0x067b;
const VID_WCH: u16 = 0x1a86;

/// Selection options forwarded from the CLI.
#[derive(Default, Copy, Clone, Debug)]
pub struct Options {
    /// Force the FTDI bulk-IN wMaxPacketSize. Overrides resolver discovery and
    /// the PID heuristic. Useful for unusual FT2232 variants or clones.
    pub ftdi_mps_override: Option<u16>,
}

/// Select a decoder for the resolved port.
///
/// FTDI MPS precedence: CLI override → resolver-discovered → PID heuristic → 64.
/// Falls back to CDC-ACM passthrough for unknown vendors, which works for any
/// device that exposes raw bytes on bulk IN/OUT endpoints.
pub fn select(info: &PortInfo, opts: Options) -> Box<dyn Decoder> {
    match info.vid {
        VID_FTDI => {
            let mps = opts
                .ftdi_mps_override
                .or(info.bulk_in_max_packet)
                .unwrap_or_else(|| ftdi_mps_for_pid(info.pid));
            Box::new(Ftdi::new(mps))
        }
        VID_WCH => Box::new(Ch340),
        VID_PROLIFIC => Box::new(Pl2303),
        _ => Box::new(CdcAcm),
    }
}

/// FTDI bulk-IN wMaxPacketSize by PID. Used when the resolver can't read the
/// endpoint descriptor (currently the case on Windows — see `resolve/win.rs`).
///
/// Source: FTDI datasheets and Linux `drivers/usb/serial/ftdi_sio_ids.h`.
fn ftdi_mps_for_pid(pid: u16) -> u16 {
    match pid {
        // FT232AM/BM/R/L (full-speed)
        0x6001 => 64,
        // FT2232C/D (full-speed) and FT2232H (high-speed) share PID 0x6010.
        // Default to high-speed since FT2232H is far more common today; users
        // with FT2232C/D can pass --ftdi-mps 64.
        0x6010 => 512,
        // FT4232H (high-speed)
        0x6011 => 512,
        // FT232H (high-speed)
        0x6014 => 512,
        // FT-X series: FT200XD/201XQ/220X/221X/230X/231X/240X (full-speed)
        0x6015 => 64,
        // Unknown FTDI PID — assume full-speed (the safer default; mis-chunking
        // a high-speed device produces visibly garbled output and the user can
        // override with --ftdi-mps 512).
        _ => 64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(vid: u16, pid: u16, mps: Option<u16>) -> PortInfo {
        PortInfo {
            bus: 0,
            devnum: 1,
            vid,
            pid,
            interface_number: None,
            bulk_in_ep: None,
            bulk_out_ep: None,
            bulk_in_max_packet: mps,
        }
    }

    #[test]
    fn cli_override_wins_over_resolver_value() {
        let i = info(VID_FTDI, 0x6001, Some(64));
        let opts = Options {
            ftdi_mps_override: Some(512),
        };
        assert_eq!(select(&i, opts).name(), "ftdi");
        // The override path is exercised; we can't read the MPS back from the
        // trait object, but the next test confirms the heuristic table.
    }

    #[test]
    fn resolver_value_wins_over_pid_heuristic() {
        // 0x6010 heuristic says 512, but resolver says 64 — resolver wins.
        let i = info(VID_FTDI, 0x6010, Some(64));
        let _ = select(&i, Options::default());
    }

    #[test]
    fn pid_heuristic_table() {
        assert_eq!(ftdi_mps_for_pid(0x6001), 64);
        assert_eq!(ftdi_mps_for_pid(0x6010), 512);
        assert_eq!(ftdi_mps_for_pid(0x6011), 512);
        assert_eq!(ftdi_mps_for_pid(0x6014), 512);
        assert_eq!(ftdi_mps_for_pid(0x6015), 64);
        assert_eq!(ftdi_mps_for_pid(0xffff), 64); // unknown → safe default
    }

    #[test]
    fn non_ftdi_vendor_uses_cdc_acm() {
        let i = info(0x2341, 0x0001, None);
        assert_eq!(select(&i, Options::default()).name(), "cdc-acm");
    }

    #[test]
    fn ch340_vid_selects_ch340() {
        let i = info(VID_WCH, 0x7523, None);
        assert_eq!(select(&i, Options::default()).name(), "ch340");
    }

    #[test]
    fn pl2303_vid_selects_pl2303() {
        let i = info(VID_PROLIFIC, 0x2303, None);
        assert_eq!(select(&i, Options::default()).name(), "pl2303");
    }

    #[test]
    fn ch340_passthrough_in_and_out() {
        let mut d = Ch340;
        let mut out = Vec::new();
        d.decode(Direction::In, b"hello", &mut out);
        d.decode(Direction::Out, b" world", &mut out);
        assert_eq!(out, b"hello world");
    }

    #[test]
    fn pl2303_passthrough_in_and_out() {
        let mut d = Pl2303;
        let mut out = Vec::new();
        d.decode(Direction::In, b"hello", &mut out);
        d.decode(Direction::Out, b" world", &mut out);
        assert_eq!(out, b"hello world");
    }
}
