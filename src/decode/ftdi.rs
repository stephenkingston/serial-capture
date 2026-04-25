use super::Decoder;
use crate::capture::Direction;

/// FTDI chips (FT232*, FT2232*, FT4232*, FT-X) prefix every wMaxPacketSize-sized
/// chunk on the bulk IN endpoint with 2 modem-status bytes. Bulk OUT is raw.
///
/// Reference: drivers/usb/serial/ftdi_sio.c::ftdi_process_read_urb in the Linux kernel.
pub struct Ftdi {
    /// wMaxPacketSize of the bulk IN endpoint. 64 for full-speed (FT232),
    /// 512 for high-speed (FT2232H/FT4232H/FT232H).
    in_packet: usize,
}

impl Ftdi {
    pub fn new(bulk_in_max_packet: u16) -> Self {
        let n = bulk_in_max_packet as usize;
        Self {
            in_packet: if n >= 4 { n } else { 64 },
        }
    }
}

impl Decoder for Ftdi {
    fn name(&self) -> &'static str {
        "ftdi"
    }

    fn decode(&mut self, dir: Direction, payload: &[u8], out: &mut Vec<u8>) {
        match dir {
            Direction::Out => out.extend_from_slice(payload),
            Direction::In => {
                let chunk = self.in_packet;
                let mut i = 0;
                while i < payload.len() {
                    let end = (i + chunk).min(payload.len());
                    if end - i >= 2 {
                        out.extend_from_slice(&payload[i + 2..end]);
                    }
                    i = end;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_is_passthrough() {
        let mut d = Ftdi::new(64);
        let mut out = Vec::new();
        d.decode(Direction::Out, b"hello", &mut out);
        assert_eq!(out, b"hello");
    }

    #[test]
    fn in_strips_two_status_bytes_per_chunk() {
        // wMaxPacketSize=4 to make the test concise: each 4-byte chunk has
        // 2 status bytes + up to 2 data bytes.
        let mut d = Ftdi::new(4);
        // [SS DD DD] [SS DD DD] -> "DDDD" + "DDDD" but max_packet is 4 so
        // each chunk is 4 bytes [SS SS DD DD] -> data "DD DD"
        let payload = [0x01, 0x60, b'a', b'b', 0x01, 0x60, b'c', b'd'];
        let mut out = Vec::new();
        d.decode(Direction::In, &payload, &mut out);
        assert_eq!(out, b"abcd");
    }

    #[test]
    fn in_handles_short_final_chunk() {
        // Trailing partial chunk that has only the 2 status bytes (no data).
        let mut d = Ftdi::new(4);
        let payload = [0x01, 0x60, b'a', b'b', 0x01, 0x60];
        let mut out = Vec::new();
        d.decode(Direction::In, &payload, &mut out);
        assert_eq!(out, b"ab");
    }

    #[test]
    fn in_drops_chunk_smaller_than_header() {
        let mut d = Ftdi::new(4);
        let payload = [0x01]; // malformed
        let mut out = Vec::new();
        d.decode(Direction::In, &payload, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn in_high_speed_512_byte_packet() {
        let mut d = Ftdi::new(512);
        // single 64-byte payload fits in one chunk -> 2 header + 62 data
        let mut payload = vec![0x01, 0x60];
        payload.extend(std::iter::repeat_n(b'X', 62));
        let mut out = Vec::new();
        d.decode(Direction::In, &payload, &mut out);
        assert_eq!(out.len(), 62);
        assert!(out.iter().all(|&b| b == b'X'));
    }
}
