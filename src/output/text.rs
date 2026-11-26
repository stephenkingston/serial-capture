use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufWriter, Write, stdout};
use std::path::Path;
use time::format_description::FormatItem;
use time::macros::format_description;
use time::{Duration, OffsetDateTime, UtcOffset};

use crate::capture::{Direction, Event};
use crate::cli::Format;

/// A burst that stalls for this long is flushed even without a newline.
const IDLE_FLUSH: Duration = Duration::milliseconds(50);
/// Never hold more than this without a newline (binary protocols).
const MAX_PENDING: usize = 4096;
const TS_FMT: &[FormatItem<'static>] = format_description!(
    "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"
);

/// Reassembles per-URB events into one event per line.
///
/// Drivers split bulk data differently. FTDI hands over a full 64-byte chunk,
/// while the Windows PL2303 driver issues one-byte reads, so a single
/// `Hello, Alice.\r\n` arrives as fourteen events and would print fourteen
/// rows. Bytes are accumulated per direction, split on `\n`, and flushed when
/// the direction changes, when the stream stalls for [`IDLE_FLUSH`], or when
/// [`MAX_PENDING`] bytes arrive without a newline. The timestamp of a line is
/// the timestamp of its first byte.
pub struct LineAssembler {
    enabled: bool,
    pending: Vec<u8>,
    dir: Direction,
    ts: OffsetDateTime,
    last_ts: OffsetDateTime,
}

impl LineAssembler {
    pub fn new(enabled: bool) -> Self {
        let now = OffsetDateTime::now_utc();
        Self {
            enabled,
            pending: Vec::new(),
            dir: Direction::In,
            ts: now,
            last_ts: now,
        }
    }

    pub fn push(&mut self, ev: Event, out: &mut Vec<Event>) {
        if !self.enabled {
            out.push(ev);
            return;
        }
        if ev.bytes.is_empty() {
            return;
        }

        if !self.pending.is_empty() && (ev.dir != self.dir || ev.ts - self.last_ts > IDLE_FLUSH) {
            self.flush(out);
        }
        if self.pending.is_empty() {
            self.dir = ev.dir;
            self.ts = ev.ts;
        }
        self.last_ts = ev.ts;
        self.pending.extend_from_slice(&ev.bytes);

        while let Some(pos) = self.pending.iter().position(|&b| b == b'\n') {
            let rest = self.pending.split_off(pos + 1);
            let line = std::mem::replace(&mut self.pending, rest);
            out.push(Event {
                ts: self.ts,
                dir: self.dir,
                bytes: line,
            });
        }
        if self.pending.len() >= MAX_PENDING {
            self.flush(out);
        }
    }

    /// Emit whatever is buffered. Call this before exiting so the tail of the
    /// stream isn't lost.
    pub fn flush(&mut self, out: &mut Vec<Event>) {
        if self.pending.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.pending);
        out.push(Event {
            ts: self.ts,
            dir: self.dir,
            bytes,
        });
    }
}

pub struct TextSink {
    file: Option<BufWriter<File>>,
    write_stdout: bool,
    format: Format,
    printable_only: bool,
    local_offset: UtcOffset,
    wrap: usize,
    assembler: LineAssembler,
    assembled: Vec<Event>,
}

impl TextSink {
    /// `path=Some` writes to that file. `path=None` writes to stdout instead.
    /// `write_stdout` controls whether stdout is used: when there is a file
    /// it acts as a tee; when there is no file it is the primary destination.
    /// Both off = events are counted but discarded.
    pub fn create(
        path: Option<&Path>,
        write_stdout: bool,
        format: Format,
        printable_only: bool,
        assemble_lines: bool,
        wrap: usize,
    ) -> Result<Self> {
        let file = match path {
            Some(p) => Some(BufWriter::new(
                File::create(p).with_context(|| format!("creating {}", p.display()))?,
            )),
            None => None,
        };
        let local_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
        Ok(Self {
            file,
            write_stdout,
            format,
            printable_only,
            local_offset,
            wrap,
            assembler: LineAssembler::new(assemble_lines),
            assembled: Vec::new(),
        })
    }

    /// Feed one captured event. Returns how many lines were written.
    pub fn push_event(&mut self, ev: Event) -> Result<usize> {
        let mut out = std::mem::take(&mut self.assembled);
        self.assembler.push(ev, &mut out);
        let mut written = 0;
        for ev in out.drain(..) {
            if self.emit(&ev)? {
                written += 1;
            }
        }
        self.assembled = out;
        Ok(written)
    }

    /// Emit any buffered partial line. Returns how many lines were written.
    pub fn flush(&mut self) -> Result<usize> {
        let mut out = std::mem::take(&mut self.assembled);
        self.assembler.flush(&mut out);
        let mut written = 0;
        for ev in out.drain(..) {
            if self.emit(&ev)? {
                written += 1;
            }
        }
        self.assembled = out;
        Ok(written)
    }

    fn emit(&mut self, ev: &Event) -> Result<bool> {
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

        for line in render_rows(&ts, arrow, &ev.bytes, self.format, self.wrap) {
            if let Some(f) = self.file.as_mut() {
                writeln!(f, "{line}").context("writing to log file")?;
            }
            if self.write_stdout {
                let mut out = stdout().lock();
                writeln!(out, "{line}").ok();
            }
        }
        if let Some(f) = self.file.as_mut() {
            f.flush().context("flushing log file")?;
        }
        Ok(true)
    }
}

/// Lay out one event. The first row carries the timestamp and direction arrow;
/// continuation rows are indented to the same column instead of repeating
/// them, so a long line reads as one event. `wrap == 0` prints the whole event
/// as a single row.
fn render_rows(ts: &str, arrow: &str, bytes: &[u8], format: Format, wrap: usize) -> Vec<String> {
    let prefix = format!("{ts}  {arrow}  ");
    let indent = " ".repeat(prefix.chars().count());
    let row_width = if wrap == 0 {
        bytes.len().max(1)
    } else {
        wrap
    };

    bytes
        .chunks(row_width)
        .enumerate()
        .map(|(i, chunk)| {
            let lead = if i == 0 { prefix.as_str() } else { indent.as_str() };
            format_line(lead, chunk, format, row_width)
        })
        .collect()
}

/// True if the payload contains at least one byte that's printable ASCII or
/// common whitespace (TAB / LF / CR). Used by `--printable-only` to skip
/// pure-binary keep-alive events without losing partially-textual ones.
fn contains_text(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .any(|&b| (0x20..=0x7e).contains(&b) || matches!(b, 0x09 | 0x0a | 0x0d))
}

fn format_line(prefix: &str, bytes: &[u8], format: Format, row_width: usize) -> String {
    let mut s = String::with_capacity(96);
    s.push_str(prefix);

    match format {
        Format::Hex => push_hex(&mut s, bytes, false, row_width),
        Format::Ascii => push_ascii(&mut s, bytes),
        Format::Both => {
            push_hex(&mut s, bytes, true, row_width);
            s.push_str("  ");
            push_ascii(&mut s, bytes);
        }
    }
    s
}

fn push_hex(s: &mut String, bytes: &[u8], pad_to_full_width: bool, row_width: usize) {
    use std::fmt::Write as _;
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{b:02x}");
    }
    if pad_to_full_width && bytes.len() < row_width {
        let missing = row_width - bytes.len();
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

    fn ev(ms: i64, dir: Direction, bytes: &[u8]) -> Event {
        Event {
            ts: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
                + Duration::milliseconds(ms),
            dir,
            bytes: bytes.to_vec(),
        }
    }

    fn collect(asm: &mut LineAssembler, events: Vec<Event>) -> Vec<Event> {
        let mut out = Vec::new();
        for e in events {
            asm.push(e, &mut out);
        }
        // End of session: emit the trailing partial line, if any.
        asm.flush(&mut out);
        out
    }

    #[test]
    fn merges_one_byte_urbs_into_a_line() {
        let mut asm = LineAssembler::new(true);
        let bytes = b"Hi, Bob.\r\n";
        let events = bytes
            .iter()
            .enumerate()
            .map(|(i, &b)| ev(i as i64, Direction::In, &[b]))
            .collect();
        let out = collect(&mut asm, events);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].bytes, b"Hi, Bob.\r\n");
        assert_eq!(out[0].ts, ev(0, Direction::In, b"").ts);
    }

    #[test]
    fn splits_two_lines_within_one_burst() {
        let mut asm = LineAssembler::new(true);
        let out = collect(&mut asm, vec![ev(0, Direction::Out, b"a\r\nb\r\n")]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].bytes, b"a\r\n");
        assert_eq!(out[1].bytes, b"b\r\n");
    }

    #[test]
    fn flushes_when_direction_changes() {
        let mut asm = LineAssembler::new(true);
        let out = collect(
            &mut asm,
            vec![
                ev(0, Direction::Out, b"abc"),
                ev(1, Direction::In, b"xyz"),
            ],
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].bytes, b"abc");
        assert_eq!(out[1].bytes, b"xyz");
    }

    #[test]
    fn flushes_a_stalled_burst() {
        let mut asm = LineAssembler::new(true);
        let out = collect(
            &mut asm,
            vec![
                ev(0, Direction::In, b"partial"),
                ev(1000, Direction::In, b"next"),
            ],
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].bytes, b"partial");
        assert_eq!(out[1].bytes, b"next");
    }

    #[test]
    fn keeps_incomplete_line_until_flush() {
        let mut asm = LineAssembler::new(true);
        let mut out = Vec::new();
        asm.push(ev(0, Direction::In, b"tail"), &mut out);
        assert!(out.is_empty());
        asm.flush(&mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].bytes, b"tail");
    }

    #[test]
    fn disabled_assembler_passes_events_through() {
        let mut asm = LineAssembler::new(false);
        let out = collect(
            &mut asm,
            vec![ev(0, Direction::In, b"H"), ev(1, Direction::In, b"i")],
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].bytes, b"H");
        assert_eq!(out[1].bytes, b"i");
    }

    #[test]
    fn long_line_wraps_without_repeating_the_timestamp() {
        let bytes: Vec<u8> = (b'a'..=b'z').chain(b'0'..=b'9').collect(); // 36 bytes
        let rows = render_rows("TS", "←", &bytes, Format::Both, 16);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].starts_with("TS  ←  "));
        // Continuation rows are padded to the same column and carry no ts/arrow.
        for row in &rows[1..] {
            assert!(row.starts_with("       "), "unexpected row: {row:?}");
            assert!(!row.contains("TS"));
            assert!(!row.contains('←'));
        }
        // The ASCII columns line up under each other (compare display columns,
        // not byte offsets: the arrow is multi-byte UTF-8).
        let column = |s: &str| s.chars().take_while(|&c| c != '|').count();
        assert_eq!(column(&rows[1]), column(&rows[0]));
    }

    #[test]
    fn wrap_zero_prints_one_row_per_event() {
        let bytes = b"Hi, Bob..............................................\r\n";
        let rows = render_rows("TS", "→", bytes, Format::Both, 0);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].starts_with("TS  →  "));
        // Single row: no hex padding, so the ASCII column follows immediately
        // after the last byte pair.
        assert!(rows[0].contains("|Hi, Bob"));
    }

    #[test]
    fn short_line_pads_the_hex_column() {
        let rows = render_rows("TS", "←", b"abc", Format::Both, 16);
        assert_eq!(rows.len(), 1);
        let ascii_at = rows[0].find('|').unwrap();
        // 16 byte slots = 47 chars for the hex column.
        let hex_start = "TS  ←  ".len();
        assert_eq!(ascii_at - hex_start, 47 + 2);
    }

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
