use anyhow::{Context, Result, anyhow, bail};
use std::os::windows::ffi::OsStrExt;
use time::{Duration as TimeDuration, OffsetDateTime};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    ReadFile,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::PCWSTR;

use super::{Direction, Event, PassiveOptions};
use crate::decode::Decoder;
use crate::output::PcapSink;
use crate::resolve::PortInfo;

/// libpcap link-layer type for USBPcap-formatted packets.
const DLT_USBPCAP: u32 = 249;

/// IOCTLs derived from `CTL_CODE(0x8001, fn, METHOD_BUFFERED, access)`.
/// See USBPcap/Public/USBPcapPublic.h.
const IOCTL_USBPCAP_SETUP_BUFFER: u32 = 0x8001_E000;
const IOCTL_USBPCAP_START_FILTERING: u32 = 0x8001_E004;

const CAPTURE_BUFFER_BYTES: u32 = 1 << 20; // 1 MB
const USBPCAP_TRANSFER_BULK: u8 = 3;

pub fn run_passive(
    info: PortInfo,
    mut decoder: Box<dyn Decoder>,
    options: PassiveOptions<'_>,
    mut on_event: impl FnMut(Event) -> Result<()>,
) -> Result<()> {
    // TODO(milestone-later): enumerate hubs via IOCTL_USBPCAP_GET_HUB_SYMLINK and
    // pick the USBPcapN that filters the target's parent root hub. For now we open
    // \\.\USBPcap1 (typical first interface). Multi-controller systems may need
    // to override this — surface a flag once we have multi-controller test data.
    let handle = open_usbpcap(r"\\.\USBPcap1")?;
    let _guard = HandleGuard(handle);

    setup_buffer(handle, CAPTURE_BUFFER_BYTES)?;
    start_filtering(handle, info.devnum)?;

    let mut header = [0u8; 24];
    read_exact(handle, &mut header).context("reading USBPcap pcap file header")?;
    let dlt = u32::from_le_bytes(header[20..24].try_into().unwrap());
    if dlt != DLT_USBPCAP {
        bail!("expected DLT_USBPCAP ({DLT_USBPCAP}) on \\\\.\\USBPcap1, got {dlt}");
    }

    let mut pcap_sink = match options.pcap_path {
        Some(p) => Some(PcapSink::create(p, DLT_USBPCAP as u16, 65535)?),
        None => None,
    };

    let mut decoded: Vec<u8> = Vec::with_capacity(4096);
    let mut pkthdr = [0u8; 16];
    let mut pkt_buf: Vec<u8> = Vec::with_capacity(65536);

    loop {
        read_exact(handle, &mut pkthdr).context("reading per-packet pcap header")?;
        let ts_sec = u32::from_le_bytes(pkthdr[0..4].try_into().unwrap()) as i64;
        let ts_usec = u32::from_le_bytes(pkthdr[4..8].try_into().unwrap()) as i64;
        let incl_len = u32::from_le_bytes(pkthdr[8..12].try_into().unwrap()) as usize;

        if incl_len > pkt_buf.capacity() {
            pkt_buf.reserve(incl_len - pkt_buf.capacity());
        }
        pkt_buf.resize(incl_len, 0);
        read_exact(handle, &mut pkt_buf).context("reading USBPcap packet body")?;

        let pkt = parse_packet(&pkt_buf);
        let pkt = match pkt {
            Some(p) => p,
            None => continue,
        };

        let ts = OffsetDateTime::from_unix_timestamp(ts_sec)
            .unwrap_or_else(|_| OffsetDateTime::now_utc())
            + TimeDuration::microseconds(ts_usec);

        // Tee URB to pcap before bulk filtering so control transfers, etc., are
        // also recorded for Wireshark — filter only by device address here since
        // USBPcap's address filter has already constrained the stream.
        if pkt.device == info.devnum as u16
            && let Some(sink) = pcap_sink.as_mut()
        {
            sink.write_packet(ts, &pkt_buf)?;
        }

        if pkt.transfer != USBPCAP_TRANSFER_BULK {
            continue;
        }
        if pkt.device != info.devnum as u16 {
            continue;
        }
        if pkt.data_length == 0 || pkt.payload.is_empty() {
            continue;
        }

        let dir_in = (pkt.endpoint & 0x80) != 0;
        if dir_in {
            if let Some(want) = info.bulk_in_ep
                && pkt.endpoint != want
            {
                continue;
            }
        } else if let Some(want) = info.bulk_out_ep
            && pkt.endpoint != want
        {
            continue;
        }

        let dir = if dir_in { Direction::In } else { Direction::Out };
        decoded.clear();
        decoder.decode(dir, pkt.payload, &mut decoded);
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

struct ParsedPacket<'a> {
    device: u16,
    endpoint: u8,
    transfer: u8,
    data_length: u32,
    payload: &'a [u8],
}

fn parse_packet(buf: &[u8]) -> Option<ParsedPacket<'_>> {
    // USBPCAP_BUFFER_PACKET_HEADER, packed:
    //  0..2   headerLen (u16)
    //  2..10  irpId (u64)
    // 10..14  status (u32)
    // 14..16  function (u16)
    // 16..17  info (u8)
    // 17..19  bus (u16)
    // 19..21  device (u16)
    // 21..22  endpoint (u8)
    // 22..23  transfer (u8)
    // 23..27  dataLength (u32)
    if buf.len() < 27 {
        return None;
    }
    let header_len = u16::from_le_bytes(buf[0..2].try_into().unwrap()) as usize;
    if header_len < 27 || header_len > buf.len() {
        return None;
    }
    let device = u16::from_le_bytes(buf[19..21].try_into().unwrap());
    let endpoint = buf[21];
    let transfer = buf[22];
    let data_length = u32::from_le_bytes(buf[23..27].try_into().unwrap());

    let payload_end = header_len + data_length as usize;
    if payload_end > buf.len() {
        return None;
    }
    Some(ParsedPacket {
        device,
        endpoint,
        transfer,
        data_length,
        payload: &buf[header_len..payload_end],
    })
}

fn open_usbpcap(path: &str) -> Result<HANDLE> {
    let wide: Vec<u16> = std::ffi::OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // USBPcap's IOCTLs (SETUP_BUFFER, START_FILTERING) declare
    // FILE_READ_ACCESS | FILE_WRITE_ACCESS in their CTL_CODE; the I/O manager
    // rejects the IOCTL with ERROR_ACCESS_DENIED unless the handle was opened
    // with both rights. The driver itself doesn't actually require an
    // Administrator token for opening — only USBPcap's installer needs that.
    let h = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .map_err(|e| anyhow!("opening {path}: {e} — is USBPcap installed and the device plugged in?"))?;
    Ok(h)
}

fn setup_buffer(handle: HANDLE, bytes: u32) -> Result<()> {
    let val = bytes;
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            handle,
            IOCTL_USBPCAP_SETUP_BUFFER,
            Some(&val as *const _ as *const _),
            std::mem::size_of::<u32>() as u32,
            None,
            0,
            Some(&mut returned),
            None,
        )
    }
    .map_err(|e| anyhow!("USBPcap SETUP_BUFFER ioctl failed: {e}"))?;
    Ok(())
}

#[repr(C)]
struct UsbpcapAddressFilter {
    addresses: [u32; 4],
    filter_all: u8,
}

fn start_filtering(handle: HANDLE, address: u8) -> Result<()> {
    let mut filter = UsbpcapAddressFilter {
        addresses: [0; 4],
        filter_all: 0,
    };
    let bit = address as u32;
    if bit < 128 {
        let word = (bit / 32) as usize;
        let mask = 1u32 << (bit % 32);
        filter.addresses[word] |= mask;
    }
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            handle,
            IOCTL_USBPCAP_START_FILTERING,
            Some(&filter as *const _ as *const _),
            std::mem::size_of::<UsbpcapAddressFilter>() as u32,
            None,
            0,
            Some(&mut returned),
            None,
        )
    }
    .map_err(|e| anyhow!("USBPcap START_FILTERING ioctl failed: {e}"))?;
    Ok(())
}

fn read_exact(handle: HANDLE, buf: &mut [u8]) -> Result<()> {
    let mut filled = 0usize;
    while filled < buf.len() {
        let mut got: u32 = 0;
        unsafe {
            ReadFile(
                handle,
                Some(&mut buf[filled..]),
                Some(&mut got),
                None,
            )
        }
        .map_err(|e| anyhow!("ReadFile on USBPcap: {e}"))?;
        if got == 0 {
            bail!("USBPcap stream closed unexpectedly");
        }
        filled += got as usize;
    }
    Ok(())
}

struct HandleGuard(HANDLE);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe { let _ = CloseHandle(self.0); }
    }
}
