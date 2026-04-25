//! Minimal pcapng writer that emits a single-section, single-interface file
//! suitable for opening in Wireshark or piping through tcpdump/capinfos.
//!
//! Format reference: <https://www.ietf.org/archive/id/draft-tuexen-opsawg-pcapng-02.html>

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use time::OffsetDateTime;

const BLOCK_TYPE_SHB: u32 = 0x0A0D_0D0A;
const BLOCK_TYPE_IDB: u32 = 0x0000_0001;
const BLOCK_TYPE_EPB: u32 = 0x0000_0006;
const BYTE_ORDER_MAGIC: u32 = 0x1A2B_3C4D;

pub struct PcapSink {
    file: BufWriter<File>,
}

impl PcapSink {
    pub fn create(path: &Path, link_type: u16, snaplen: u32) -> Result<Self> {
        let f = File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let mut s = Self {
            file: BufWriter::new(f),
        };
        s.write_section_header()?;
        s.write_interface_description(link_type, snaplen)?;
        s.file.flush().context("flushing pcapng header")?;
        Ok(s)
    }

    /// Write the raw link-layer packet bytes as captured. `ts` is the packet's
    /// timestamp; we encode it as microseconds since the Unix epoch (the IDB
    /// default `if_tsresol` value).
    pub fn write_packet(&mut self, ts: OffsetDateTime, packet: &[u8]) -> Result<()> {
        let captured = packet.len() as u32;
        let original = captured;
        let micros: u64 = ts
            .unix_timestamp_nanos()
            .max(0)
            .div_euclid(1_000)
            .try_into()
            .unwrap_or(0);
        let ts_high = (micros >> 32) as u32;
        let ts_low = (micros & 0xFFFF_FFFF) as u32;

        let pad = (4 - (captured & 3)) & 3;
        let total_len: u32 = 32 + captured + pad;

        let f = &mut self.file;
        f.write_all(&BLOCK_TYPE_EPB.to_le_bytes())?;
        f.write_all(&total_len.to_le_bytes())?;
        f.write_all(&0u32.to_le_bytes())?; // interface_id = 0
        f.write_all(&ts_high.to_le_bytes())?;
        f.write_all(&ts_low.to_le_bytes())?;
        f.write_all(&captured.to_le_bytes())?;
        f.write_all(&original.to_le_bytes())?;
        f.write_all(packet)?;
        if pad > 0 {
            f.write_all(&[0u8; 3][..pad as usize])?;
        }
        f.write_all(&total_len.to_le_bytes())?;
        f.flush().context("flushing pcapng EPB")?;
        Ok(())
    }

    fn write_section_header(&mut self) -> Result<()> {
        let total_len: u32 = 28;
        let f = &mut self.file;
        f.write_all(&BLOCK_TYPE_SHB.to_le_bytes())?;
        f.write_all(&total_len.to_le_bytes())?;
        f.write_all(&BYTE_ORDER_MAGIC.to_le_bytes())?;
        f.write_all(&1u16.to_le_bytes())?; // major
        f.write_all(&0u16.to_le_bytes())?; // minor
        f.write_all(&(-1i64).to_le_bytes())?; // section_length unknown
        f.write_all(&total_len.to_le_bytes())?;
        Ok(())
    }

    fn write_interface_description(&mut self, link_type: u16, snaplen: u32) -> Result<()> {
        let total_len: u32 = 20;
        let f = &mut self.file;
        f.write_all(&BLOCK_TYPE_IDB.to_le_bytes())?;
        f.write_all(&total_len.to_le_bytes())?;
        f.write_all(&link_type.to_le_bytes())?;
        f.write_all(&0u16.to_le_bytes())?; // reserved
        f.write_all(&snaplen.to_le_bytes())?;
        f.write_all(&total_len.to_le_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn shb_layout() {
        let dir = tempdir();
        let path = dir.join("test.pcapng");
        let _ = PcapSink::create(&path, 220, 65535).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        // 28-byte SHB + 20-byte IDB = 48 bytes total
        assert_eq!(bytes.len(), 48);
        // SHB type, length, magic
        assert_eq!(&bytes[0..4], &0x0A0D_0D0A_u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &28u32.to_le_bytes());
        assert_eq!(&bytes[8..12], &0x1A2B_3C4D_u32.to_le_bytes());
        // SHB trailing length
        assert_eq!(&bytes[24..28], &28u32.to_le_bytes());
        // IDB type, length, link_type
        assert_eq!(&bytes[28..32], &0x0000_0001_u32.to_le_bytes());
        assert_eq!(&bytes[32..36], &20u32.to_le_bytes());
        assert_eq!(&bytes[36..38], &220u16.to_le_bytes());
    }

    #[test]
    fn epb_payload_and_padding() {
        let dir = tempdir();
        let path = dir.join("test.pcapng");
        let mut s = PcapSink::create(&path, 220, 65535).unwrap();
        // 5-byte payload requires 3 bytes of padding to reach a 4-byte boundary.
        s.write_packet(datetime!(2026-04-25 12:30:45 UTC), &[1, 2, 3, 4, 5])
            .unwrap();
        drop(s);
        let bytes = std::fs::read(&path).unwrap();
        // Header (48) + EPB
        assert!(bytes.len() > 48);
        let epb = &bytes[48..];
        // Block type 0x06, total length = 32 + 5 + 3 = 40
        assert_eq!(&epb[0..4], &0x0000_0006_u32.to_le_bytes());
        assert_eq!(&epb[4..8], &40u32.to_le_bytes());
        // captured_len and original_len both 5
        assert_eq!(&epb[20..24], &5u32.to_le_bytes());
        assert_eq!(&epb[24..28], &5u32.to_le_bytes());
        // payload bytes 1..=5 followed by 3 padding zero bytes
        assert_eq!(&epb[28..33], &[1, 2, 3, 4, 5]);
        assert_eq!(&epb[33..36], &[0, 0, 0]);
        // Trailing length matches
        assert_eq!(&epb[36..40], &40u32.to_le_bytes());
    }

    #[test]
    fn epb_no_padding_when_aligned() {
        let dir = tempdir();
        let path = dir.join("test.pcapng");
        let mut s = PcapSink::create(&path, 220, 65535).unwrap();
        let payload = [0xAAu8; 8];
        s.write_packet(datetime!(2026-04-25 0:0:0 UTC), &payload)
            .unwrap();
        drop(s);
        let bytes = std::fs::read(&path).unwrap();
        let epb = &bytes[48..];
        // total = 32 + 8 + 0 = 40
        assert_eq!(&epb[4..8], &40u32.to_le_bytes());
        // first trailing length at offset 36
        assert_eq!(&epb[36..40], &40u32.to_le_bytes());
    }

    fn tempdir() -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.push(format!("serial-capture-test-{token}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
