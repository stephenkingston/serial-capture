use super::Decoder;
use crate::capture::Direction;

/// Silicon Labs CP210x family USB-serial bridges (CP2102, CP2102N, CP2103,
/// CP2104, CP2105, CP2108, CP2109).
///
/// Bulk IN and OUT carry raw serial bytes with no per-packet wrapper. Baud,
/// line coding, and modem-control state are configured via vendor-specific
/// control transfers (bRequest 0x00..0x1E), which we ignore for capture
/// purposes.
///
/// Note: multi-UART parts (CP2105 dual, CP2108 quad) expose one USB interface
/// and bulk endpoint pair per UART. On Linux the resolver picks the endpoints
/// of the interface backing the requested tty, so streams stay separate. On
/// Windows endpoint discovery is not implemented yet, so capture is filtered
/// by device address only and traffic from all UARTs of one chip is merged.
pub struct Cp210x;

impl Decoder for Cp210x {
    fn name(&self) -> &'static str {
        "cp210x"
    }

    fn decode(&mut self, _dir: Direction, payload: &[u8], out: &mut Vec<u8>) {
        out.extend_from_slice(payload);
    }
}
