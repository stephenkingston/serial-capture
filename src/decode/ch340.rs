use super::Decoder;
use crate::capture::Direction;

/// WCH CH340/CH341/CH343/CH9102 USB-serial bridges.
///
/// Bulk IN and OUT carry raw serial bytes with no per-packet wrapper. Baud,
/// parity, and modem-control state are configured via vendor-specific control
/// transfers, which we ignore for capture purposes.
pub struct Ch340;

impl Decoder for Ch340 {
    fn name(&self) -> &'static str {
        "ch340"
    }

    fn decode(&mut self, _dir: Direction, payload: &[u8], out: &mut Vec<u8>) {
        out.extend_from_slice(payload);
    }
}
