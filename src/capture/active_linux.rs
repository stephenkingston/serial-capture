//! Active proxy mode (Linux): bridge a pty pair to a real serial port and
//! tee bytes to the TextSink.
//!
//! The user's application connects to the printed pty path; we forward its
//! bytes to the real device and the device's bytes back, logging both
//! directions. There's no USB-level capture in this mode, so `--pcap` is not
//! supported here.

use anyhow::{Context, Result, anyhow};
use nix::fcntl::OFlag;
use nix::libc;
use nix::pty::{grantpt, posix_openpt, ptsname_r, unlockpt};
use nix::sys::termios::{
    BaudRate, ControlFlags, InputFlags, LocalFlags, OutputFlags, SetArg, cfsetspeed, tcgetattr,
    tcsetattr,
};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsFd, FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::BorrowedFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use time::OffsetDateTime;

use super::{Direction, Event};
use crate::output::TextSink;

pub fn run_active(
    real_port: &str,
    baud: Option<u32>,
    sink: Arc<std::sync::Mutex<TextSink>>,
    written: Arc<AtomicUsize>,
) -> Result<()> {
    let master = posix_openpt(OFlag::O_RDWR | OFlag::O_NOCTTY)
        .map_err(|e| anyhow!("posix_openpt: {e}"))?;
    grantpt(&master).map_err(|e| anyhow!("grantpt: {e}"))?;
    unlockpt(&master).map_err(|e| anyhow!("unlockpt: {e}"))?;
    // Put the pty into raw mode so the line discipline doesn't insert \r,
    // mangle CR/LF, or echo bytes back into the master (which would otherwise
    // create a loop where everything we write to master gets re-read).
    set_raw(master.as_fd()).context("setting pty master to raw mode")?;
    let slave_path =
        ptsname_r(&master).map_err(|e| anyhow!("ptsname_r: {e}"))?;

    eprintln!("→ Connect your application to {slave_path}");
    eprintln!("  (mirrors {real_port})");
    if let Some(b) = baud {
        eprintln!("→ Real port baud: {b}");
    }
    eprintln!("→ Press Ctrl-C to stop.");

    let real = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(real_port)
        .with_context(|| format!("opening real port {real_port}"))?;

    if let Some(b) = baud {
        configure_baud(real.as_fd(), b)
            .with_context(|| format!("configuring baud {b} on {real_port}"))?;
    }
    set_raw(real.as_fd()).context("setting real port to raw mode")?;

    // Duplicate FDs so each forwarder thread owns one read + one write end.
    let master_fd: OwnedFd = master.into();
    let master_for_real_to_pty = master_fd.try_clone().context("dup master fd")?;
    let real_for_pty_to_real = real.try_clone().context("dup real fd")?;

    // Wrap raw fds as Files so we can use std Read/Write. From here each File
    // owns its descriptor and will close on drop.
    let master_read = unsafe { File::from_raw_fd(master_fd.into_raw_fd()) };
    let master_write = unsafe {
        File::from_raw_fd(master_for_real_to_pty.into_raw_fd())
    };
    let real_write = real;
    let real_read = real_for_pty_to_real;

    // Direction nomenclature: Out = host application → device (via pty master).
    //                         In  = device → host application.
    let pty_to_real = thread::Builder::new()
        .name("pty→real".into())
        .spawn({
            let sink = sink.clone();
            let written = written.clone();
            move || forward(master_read, real_write, Direction::Out, sink, written)
        })?;

    let real_to_pty = thread::Builder::new()
        .name("real→pty".into())
        .spawn({
            let sink = sink.clone();
            let written = written.clone();
            move || forward(real_read, master_write, Direction::In, sink, written)
        })?;

    // If either thread exits (typically because its read returned 0 / EIO when
    // the other side closes), we tear down. Joining gives us the chance to
    // surface errors.
    let r1 = pty_to_real.join().map_err(|e| anyhow!("pty→real thread panicked: {e:?}"))?;
    let r2 = real_to_pty.join().map_err(|e| anyhow!("real→pty thread panicked: {e:?}"))?;
    r1.or(r2)
}

fn forward(
    mut src: File,
    mut dst: File,
    dir: Direction,
    sink: Arc<std::sync::Mutex<TextSink>>,
    written: Arc<AtomicUsize>,
) -> Result<()> {
    let mut buf = vec![0u8; 4096];
    loop {
        let n = match src.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            // EIO on a pty master happens when the slave end is closed. End the
            // forwarder cleanly so the other direction can also wind down.
            Err(e) if e.raw_os_error() == Some(libc::EIO) => return Ok(()),
            Err(e) => return Err(e).context("read from src"),
        };
        dst.write_all(&buf[..n]).context("write to dst")?;
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

fn baud_to_rate(b: u32) -> Option<BaudRate> {
    Some(match b {
        50 => BaudRate::B50,
        75 => BaudRate::B75,
        110 => BaudRate::B110,
        134 => BaudRate::B134,
        150 => BaudRate::B150,
        200 => BaudRate::B200,
        300 => BaudRate::B300,
        600 => BaudRate::B600,
        1200 => BaudRate::B1200,
        1800 => BaudRate::B1800,
        2400 => BaudRate::B2400,
        4800 => BaudRate::B4800,
        9600 => BaudRate::B9600,
        19200 => BaudRate::B19200,
        38400 => BaudRate::B38400,
        57600 => BaudRate::B57600,
        115200 => BaudRate::B115200,
        230400 => BaudRate::B230400,
        460800 => BaudRate::B460800,
        500000 => BaudRate::B500000,
        921600 => BaudRate::B921600,
        1000000 => BaudRate::B1000000,
        1500000 => BaudRate::B1500000,
        2000000 => BaudRate::B2000000,
        _ => return None,
    })
}

fn configure_baud(fd: BorrowedFd<'_>, b: u32) -> Result<()> {
    let rate = baud_to_rate(b)
        .ok_or_else(|| anyhow!("unsupported baud rate {b} (try a standard rate like 115200)"))?;
    let mut t = tcgetattr(fd).map_err(|e| anyhow!("tcgetattr: {e}"))?;
    cfsetspeed(&mut t, rate).map_err(|e| anyhow!("cfsetspeed: {e}"))?;
    tcsetattr(fd, SetArg::TCSANOW, &t).map_err(|e| anyhow!("tcsetattr: {e}"))?;
    Ok(())
}

fn set_raw(fd: BorrowedFd<'_>) -> Result<()> {
    let mut t = tcgetattr(fd).map_err(|e| anyhow!("tcgetattr: {e}"))?;
    t.input_flags.remove(
        InputFlags::IGNBRK
            | InputFlags::BRKINT
            | InputFlags::PARMRK
            | InputFlags::ISTRIP
            | InputFlags::INLCR
            | InputFlags::IGNCR
            | InputFlags::ICRNL
            | InputFlags::IXON,
    );
    t.output_flags.remove(OutputFlags::OPOST);
    t.local_flags.remove(
        LocalFlags::ECHO
            | LocalFlags::ECHONL
            | LocalFlags::ICANON
            | LocalFlags::ISIG
            | LocalFlags::IEXTEN,
    );
    t.control_flags.remove(ControlFlags::CSIZE | ControlFlags::PARENB);
    t.control_flags.insert(ControlFlags::CS8);
    tcsetattr(fd, SetArg::TCSANOW, &t).map_err(|e| anyhow!("tcsetattr: {e}"))?;
    Ok(())
}

