use super::Decoder;
use crate::capture::Direction;

/// CDC-ACM and any chip whose bulk IN/OUT endpoints carry raw serial bytes
/// without a wrapper (e.g. CH340, PL2303 in normal operation).
pub struct CdcAcm;

impl Decoder for CdcAcm {
    fn name(&self) -> &'static str {
        "cdc-acm"
    }

    fn decode(&mut self, _dir: Direction, payload: &[u8], out: &mut Vec<u8>) {
        out.extend_from_slice(payload);
    }
}
