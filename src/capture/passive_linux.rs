use anyhow::{Context, Result, bail};
use time::OffsetDateTime;

use super::{Direction, Event, PassiveOptions};
use crate::decode::Decoder;
use crate::output::PcapSink;
use crate::resolve::PortInfo;

/// usbmon binary header link types reported by libpcap.
const DLT_USB_LINUX: i32 = 189;
const DLT_USB_LINUX_MMAPPED: i32 = 220;

/// usbmon `type` byte values (from `Documentation/usb/usbmon.rst`).
const TYPE_SUBMIT: u8 = b'S';
const TYPE_CALLBACK: u8 = b'C';

/// usbmon `xfer_type` byte values.
const XFER_BULK: u8 = 3;

/// Open usbmonN for the device's bus and stream serial events to `on_event`
/// until the capture is interrupted (Ctrl-C closes the process).
pub fn run_passive(
    info: PortInfo,
    mut decoder: Box<dyn Decoder>,
    options: PassiveOptions<'_>,
    mut on_event: impl FnMut(Event) -> Result<()>,
) -> Result<()> {
    let iface = format!("usbmon{}", info.bus);

    preflight(info.bus)?;

    let cap = pcap::Capture::from_device(iface.as_str())
        .with_context(|| format!("opening capture device '{iface}'"))?
        .immediate_mode(true)
        .snaplen(65535)
        .open()
        .with_context(|| format!("opening '{iface}' for capture"))?;

    let linktype = cap.get_datalink().0;
    let header_len: usize = match linktype {
        DLT_USB_LINUX => 48,
        DLT_USB_LINUX_MMAPPED => 64,
        other => bail!("unexpected link type {other} on '{iface}' — expected usbmon"),
    };

    let mut pcap_sink = match options.pcap_path {
        Some(p) => Some(PcapSink::create(p, linktype as u16, 65535)?),
        None => None,
    };

    let mut cap = cap;
    let mut decoded = Vec::with_capacity(4096);

    loop {
        let pkt = match cap.next_packet() {
            Ok(p) => p,
            Err(pcap::Error::TimeoutExpired) => continue,
            Err(e) => return Err(e).context("reading from usbmon"),
        };

        let data = pkt.data;
        if data.len() < header_len {
            continue;
        }

        let h = parse_header(data);

        if h.busnum != info.bus || h.devnum != info.devnum {
            continue;
        }

        let ts = OffsetDateTime::from_unix_timestamp(h.ts_sec)
            .unwrap_or_else(|_| OffsetDateTime::now_utc())
            + time::Duration::microseconds(h.ts_usec as i64);

        // Tee the full URB record (including any control transfers) to pcap so
        // Wireshark sees the device's complete USB activity.
        if let Some(sink) = pcap_sink.as_mut() {
            sink.write_packet(ts, data)?;
        }

        if h.xfer_type != XFER_BULK {
            continue;
        }

        let dir_in = (h.epnum & 0x80) != 0;
        if dir_in {
            if let Some(want) = info.bulk_in_ep
                && h.epnum != want
            {
                continue;
            }
        } else if let Some(want) = info.bulk_out_ep
            && h.epnum != want
        {
            continue;
        }
        // OUT bytes ride on the submit; IN bytes ride on the callback.
        let has_data = match (h.type_, dir_in) {
            (TYPE_SUBMIT, false) => true,
            (TYPE_CALLBACK, true) => true,
            _ => false,
        };
        if !has_data {
            continue;
        }

        let payload_end = header_len + h.len_cap as usize;
        if h.len_cap == 0 || payload_end > data.len() {
            continue;
        }
        let payload = &data[header_len..payload_end];

        let dir = if dir_in { Direction::In } else { Direction::Out };
        decoded.clear();
        decoder.decode(dir, payload, &mut decoded);
        if decoded.is_empty() {
            continue;
        }

        on_event(Event {
            ts,
            dir,
            bytes: std::mem::take(&mut decoded),
        })?;
        decoded = Vec::with_capacity(4096);
    }
}

struct Header {
    type_: u8,
    xfer_type: u8,
    epnum: u8,
    devnum: u8,
    busnum: u16,
    ts_sec: i64,
    ts_usec: i32,
    len_cap: u32,
}

fn preflight(bus: u16) -> Result<()> {
    // main.rs runs install::linux_usbmon::ensure_ready before we get here.
    // This is just a final sanity check.
    let dev_path = format!("/dev/usbmon{bus}");
    if std::fs::File::open(&dev_path).is_err() {
        bail!(
            "cannot open {dev_path} — re-run with --yes for auto-setup, \
             or load usbmon and grant access manually."
        );
    }
    Ok(())
}

fn parse_header(buf: &[u8]) -> Header {
    Header {
        type_: buf[8],
        xfer_type: buf[9],
        epnum: buf[10],
        devnum: buf[11],
        busnum: u16::from_le_bytes(buf[12..14].try_into().unwrap()),
        ts_sec: i64::from_le_bytes(buf[16..24].try_into().unwrap()),
        ts_usec: i32::from_le_bytes(buf[24..28].try_into().unwrap()),
        len_cap: u32::from_le_bytes(buf[36..40].try_into().unwrap()),
    }
}
