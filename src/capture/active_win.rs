//! Active proxy mode (Windows): bridge a com0com pair end to a real COM port
//! and tee bytes to the TextSink.
//!
//! Default com0com pair names are CNCA0 / CNCB0. The user's application
//! connects to CNCA0; we hold CNCB0 and forward bytes to/from the real port.
//! Pair management (rename to COM20/COM21, etc.) is done by the user via
//! setupc — this code uses whichever pair end is given as `our_end`.

use anyhow::{Context, Result, anyhow};
use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;
use time::OffsetDateTime;

use super::{Direction, Event};
use crate::output::TextSink;

const DEFAULT_OUR_END: &str = "CNCB0";
const DEFAULT_APP_END: &str = "CNCA0";

pub fn run_active(
    real_port: &str,
    baud: Option<u32>,
    sink: Arc<std::sync::Mutex<TextSink>>,
    written: Arc<AtomicUsize>,
) -> Result<()> {
    let baud = baud.unwrap_or(9600);

    eprintln!("→ Connect your application to {DEFAULT_APP_END}");
    eprintln!("  (mirrors {real_port})");
    eprintln!("→ Real port baud: {baud}");
    eprintln!("→ Press Ctrl-C to stop.");

    let real_a = open_port(real_port, baud)
        .with_context(|| format!("opening real port {real_port}"))?;
    let real_b = real_a
        .try_clone()
        .with_context(|| format!("cloning real port handle {real_port}"))?;

    let pair_a = open_port(DEFAULT_OUR_END, baud)
        .with_context(|| format!(
            "opening com0com pair end {DEFAULT_OUR_END} — \
             is the default pair installed? Run setupc.exe list to verify."
        ))?;
    let pair_b = pair_a
        .try_clone()
        .with_context(|| format!("cloning com0com pair end {DEFAULT_OUR_END}"))?;

    // Direction nomenclature: Out = host application → device.
    //                         In  = device → host application.
    let pair_to_real = thread::Builder::new()
        .name("pair→real".into())
        .spawn({
            let sink = sink.clone();
            let written = written.clone();
            move || forward(pair_a, real_a, Direction::Out, sink, written)
        })?;

    let real_to_pair = thread::Builder::new()
        .name("real→pair".into())
        .spawn({
            let sink = sink.clone();
            let written = written.clone();
            move || forward(real_b, pair_b, Direction::In, sink, written)
        })?;

    let r1 = pair_to_real
        .join()
        .map_err(|e| anyhow!("pair→real thread panicked: {e:?}"))?;
    let r2 = real_to_pair
        .join()
        .map_err(|e| anyhow!("real→pair thread panicked: {e:?}"))?;
    r1.or(r2)
}

fn open_port(name: &str, baud: u32) -> Result<Box<dyn SerialPort>> {
    let port = serialport::new(name, baud)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .flow_control(FlowControl::None)
        // Short read timeout so threads can wake periodically. On clean process
        // exit (Ctrl-C) the OS closes handles and reads return; this timeout
        // also makes the bridge resilient to the other side closing without
        // an immediate signal.
        .timeout(Duration::from_millis(100))
        .open()
        .map_err(|e| anyhow!("opening {name}: {e}"))?;
    Ok(port)
}

fn forward(
    mut src: Box<dyn SerialPort>,
    mut dst: Box<dyn SerialPort>,
    dir: Direction,
    sink: Arc<std::sync::Mutex<TextSink>>,
    written: Arc<AtomicUsize>,
) -> Result<()> {
    let mut buf = vec![0u8; 4096];
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(anyhow!("read: {e}")),
        };
        dst.write_all(&buf[..n])
            .map_err(|e| anyhow!("write: {e}"))?;
        let event = Event {
            ts: OffsetDateTime::now_utc(),
            dir,
            bytes: buf[..n].to_vec(),
        };
        let mut sink = sink.lock().expect("sink mutex poisoned");
        if sink.write_event(&event)? {
            written.fetch_add(1, Ordering::Relaxed);
        }
    }
}
