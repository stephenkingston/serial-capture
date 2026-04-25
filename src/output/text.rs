use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufWriter, Write, stdout};
use std::path::Path;
use time::UtcOffset;
use time::format_description::FormatItem;
use time::macros::format_description;

use crate::capture::{Direction, Event};
use crate::cli::Format;

const BYTES_PER_LINE: usize = 16;
const TS_FMT: &[FormatItem<'static>] = format_description!(
    "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"
);

pub struct TextSink {
    file: BufWriter<File>,
    tee_stdout: bool,
    format: Format,
    printable_only: bool,
    local_offset: UtcOffset,
}

impl TextSink {
    pub fn create(
        path: &Path,
        tee_stdout: bool,
        format: Format,
        printable_only: bool,
    ) -> Result<Self> {
        let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let local_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
        Ok(Self {
            file: BufWriter::new(file),
            tee_stdout,
            format,
            printable_only,
            local_offset,
        })
    }

    pub fn write_event(&mut self, ev: &Event) -> Result<bool> {
        if self.printable_only && !contains_text(&ev.bytes) {
            return Ok(false);
        }
        let ts = ev
            .ts
            .to_offset(self.local_offset)
            .format(TS_FMT)
            .unwrap_or_else(|_| String::from("?"));
        let arrow = match ev.dir {
            Direction::Out => "→",
            Direction::In => "←",
        };

        for chunk in ev.bytes.chunks(BYTES_PER_LINE) {
            let line = format_line(&ts, arrow, chunk, self.format);
            writeln!(self.file, "{line}").context("writing to log file")?;
            if self.tee_stdout {
                let mut out = stdout().lock();
                writeln!(out, "{line}").ok();
            }
        }
        self.file.flush().context("flushing log file")?;
        Ok(true)
    }
}

/// True if the payload contains at least one byte that's printable ASCII or
/// common whitespace (TAB / LF / CR). Used by `--printable-only` to skip
/// pure-binary keep-alive events without losing partially-textual ones.
fn contains_text(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .any(|&b| (0x20..=0x7e).contains(&b) || matches!(b, 0x09 | 0x0a | 0x0d))
}

fn format_line(ts: &str, arrow: &str, bytes: &[u8], format: Format) -> String {
    let mut s = String::with_capacity(96);
    s.push_str(ts);
    s.push_str("  ");
    s.push_str(arrow);
    s.push_str("  ");

    match format {
        Format::Hex => push_hex(&mut s, bytes, false),
        Format::Ascii => push_ascii(&mut s, bytes),
        Format::Both => {
            push_hex(&mut s, bytes, true);
            s.push_str("  ");
            push_ascii(&mut s, bytes);
        }
    }
    s
}

fn push_hex(s: &mut String, bytes: &[u8], pad_to_full_width: bool) {
    use std::fmt::Write as _;
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{b:02x}");
    }
    if pad_to_full_width && bytes.len() < BYTES_PER_LINE {
        let missing = BYTES_PER_LINE - bytes.len();
        // each missing byte = "XX " (3 chars); the trailing space becomes the gap.
        for _ in 0..missing {
            s.push_str("   ");
        }
    }
}

fn push_ascii(s: &mut String, bytes: &[u8]) {
    s.push('|');
    for &b in bytes {
        let c = if (0x20..=0x7e).contains(&b) {
            b as char
        } else {
            '.'
        };
        s.push(c);
    }
    s.push('|');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_text_detects_printable_ascii() {
        assert!(contains_text(b"hello"));
        assert!(contains_text(b"x"));
    }

    #[test]
    fn contains_text_keeps_partial_text() {
        // payload with mostly binary plus one printable byte still passes
        let payload = [0x00, 0x01, 0xff, b'A', 0xfe, 0x80];
        assert!(contains_text(&payload));
    }

    #[test]
    fn contains_text_keeps_common_whitespace() {
        assert!(contains_text(&[b'\t']));
        assert!(contains_text(&[b'\n']));
        assert!(contains_text(&[b'\r']));
        assert!(contains_text(b"\r\n"));
    }

    #[test]
    fn contains_text_rejects_pure_binary() {
        let payload = [0x00, 0x01, 0x02, 0xff, 0xfe, 0x80];
        assert!(!contains_text(&payload));
    }

    #[test]
    fn contains_text_rejects_empty() {
        assert!(!contains_text(b""));
    }
}
