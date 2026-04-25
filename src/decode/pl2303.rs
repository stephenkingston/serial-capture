use super::Decoder;
use crate::capture::Direction;

/// Prolific PL2303 family USB-serial bridges (PL2303HX/HXD/EA/RA/TA, PL2303GC).
///
/// Bulk IN and OUT carry raw serial bytes; configuration uses vendor control
/// transfers. No per-URB wrapper to strip.
pub struct Pl2303;

impl Decoder for Pl2303 {
    fn name(&self) -> &'static str {
        "pl2303"
    }

    fn decode(&mut self, _dir: Direction, payload: &[u8], out: &mut Vec<u8>) {
        out.extend_from_slice(payload);
    }
}
