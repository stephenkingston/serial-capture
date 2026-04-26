#!/usr/bin/env python3
"""serial-capture Windows end-to-end test suite (Arduino Uno).

Runs the built serial-capture.exe against a real Uno flashed with
tests/uno_firmware/uno_firmware.ino. Verifies passive-mode capture
produces expected bytes for a battery of patterns.

See tests/README.md for setup. Quick start:

    cargo build --release
    pip install pyserial
    python tests\\run_tests.py
"""

from __future__ import annotations

import argparse
import contextlib
import os
import re
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path
from queue import Queue, Empty

try:
    import serial
    import serial.tools.list_ports
except ImportError:
    sys.exit("Missing dependency: pip install pyserial")


REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_BIN = REPO_ROOT / "target" / "release" / (
    "serial-capture.exe" if os.name == "nt" else "serial-capture"
)
ARTIFACTS = REPO_ROOT / "tests" / "_artifacts"
BAUD = 115200

# VIDs that commonly back an "Arduino Uno" board (genuine + clones).
KNOWN_VIDS = {
    0x2341,  # Arduino LLC (genuine Uno R3)
    0x2A03,  # Arduino.org
    0x1A86,  # WCH (CH340 / CH341 clones)
    0x0403,  # FTDI (FT232 clones)
    0x10C4,  # Silicon Labs (CP210x clones)
    0x067B,  # Prolific (PL2303 clones)
    0x1B4F,  # SparkFun
}


# ---------------------------------------------------------------------------
# subprocess plumbing

def _drain(stream, q: "Queue[str]") -> None:
    try:
        for line in iter(stream.readline, ""):
            q.put(line)
    except Exception as e:
        q.put(f"<reader thread error: {e!r}>\n")
    finally:
        try:
            stream.close()
        except Exception:
            pass


def start_capture(bin_path: Path, port: str, log_path: Path, *extra: str):
    # --yes auto-confirms Linux sudo prompts (load usbmon, chmod /dev/usbmon*).
    # No-op when setup is already complete; on Windows it's silently ignored
    # by the install path. Without it the binary would block on stdin and the
    # reader thread would hang.
    cmd = [str(bin_path), "--port", port, "-o", str(log_path), "--yes", *extra]
    creationflags = (
        subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0
    )
    # Force UTF-8 so Rust's '→'/'←' decode cleanly on cp1252 Windows; replace
    # on bad bytes so a single odd byte can't kill the reader thread silently.
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        encoding="utf-8",
        errors="replace",
        bufsize=1,
        creationflags=creationflags,
    )
    q: "Queue[str]" = Queue()
    threading.Thread(target=_drain, args=(proc.stdout, q), daemon=True).start()
    return proc, q


def await_capture_active(
    proc, q: Queue, sink: list[str], timeout: float = 15.0
) -> bool:
    """Block until 'Logging to' has been printed (capture is recording).

    Consumed lines are appended to `sink` so post-hoc stdout assertions
    can still see the startup banner.
    """
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if proc.poll() is not None:
            return False
        try:
            line = q.get(timeout=0.2)
        except Empty:
            continue
        sink.append(line)
        if "Logging to" in line:
            time.sleep(0.3)  # brief settle so the IOCTL chain is fully armed
            return True
    return False


def stop_capture(proc, q: Queue, drain_timeout: float = 5.0) -> str:
    if proc.poll() is None:
        try:
            if os.name == "nt":
                proc.send_signal(signal.CTRL_BREAK_EVENT)
            else:
                proc.send_signal(signal.SIGINT)
        except (OSError, ValueError):
            pass
    try:
        proc.wait(timeout=drain_timeout)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=2)
    out: list[str] = []
    while True:
        try:
            out.append(q.get_nowait())
        except Empty:
            break
    return "".join(out)


class Session:
    def __init__(self, proc, q):
        self.proc = proc
        self.q = q
        self._preamble: list[str] = []
        self.output = ""
        self._stopped = False

    def stop(self) -> str:
        if not self._stopped:
            tail = stop_capture(self.proc, self.q)
            self.output = "".join(self._preamble) + tail
            self._stopped = True
        return self.output


@contextlib.contextmanager
def capture_session(bin_path: Path, port: str, log_path: Path, *extra: str):
    proc, q = start_capture(bin_path, port, log_path, *extra)
    sess = Session(proc, q)
    try:
        if not await_capture_active(proc, q, sess._preamble):
            # capture failed to start — drain stdout for context, then fail
            sess.stop()
            rc = sess.proc.returncode
            raise AssertionError(
                f"capture did not start (process exited with code {rc!r}). "
                f"stdout/stderr below:\n----\n{sess.output}----"
            )
        yield sess
    finally:
        sess.stop()


# ---------------------------------------------------------------------------
# log parsing

ARROW_H2D = "→"  # right arrow (host to device)
ARROW_D2H = "←"  # left arrow  (device to host)
HEX_CHARS = set("0123456789abcdefABCDEF")


def parse_log(text: str) -> tuple[bytes, bytes]:
    """Return (host_to_device_bytes, device_to_host_bytes) from default-format log."""
    h2d = bytearray()
    d2h = bytearray()
    for line in text.splitlines():
        idx_h = line.find(ARROW_H2D)
        idx_d = line.find(ARROW_D2H)
        if idx_h >= 0:
            target, idx, arrow = h2d, idx_h, ARROW_H2D
        elif idx_d >= 0:
            target, idx, arrow = d2h, idx_d, ARROW_D2H
        else:
            continue
        rest = line[idx + len(arrow):]
        pipe = rest.find("|")
        hex_part = rest[:pipe] if pipe >= 0 else rest
        hex_clean = "".join(c for c in hex_part if c in HEX_CHARS)
        if not hex_clean or len(hex_clean) % 2:
            continue
        try:
            target.extend(bytes.fromhex(hex_clean))
        except ValueError:
            pass
    return bytes(h2d), bytes(d2h)


# ---------------------------------------------------------------------------
# port helpers

def autodetect_port() -> str | None:
    for p in serial.tools.list_ports.comports():
        if p.vid in KNOWN_VIDS:
            return p.device
    return None


def open_uno(port: str) -> serial.Serial:
    """Open the Uno; absorb the post-DTR-reset banner."""
    s = serial.Serial(port, BAUD, timeout=1.0)
    time.sleep(2.0)              # AVR boot + bootloader timeout
    s.reset_input_buffer()
    return s


def verify_firmware(port: str) -> str:
    with serial.Serial(port, BAUD, timeout=2.0) as s:
        time.sleep(2.0)
        s.reset_input_buffer()
        s.write(b"BANNER\n")
        s.flush()
        deadline = time.monotonic() + 3
        buf = bytearray()
        while time.monotonic() < deadline and b"\n" not in buf:
            buf.extend(s.read(128))
    return buf.decode("utf-8", errors="replace").strip()


# ---------------------------------------------------------------------------
# tests

def t_passive_smoke(ctx) -> None:
    """Capture starts, runs, and exits cleanly with a 'Stopped. Wrote N' summary."""
    log = ARTIFACTS / "smoke.txt"
    log.unlink(missing_ok=True)
    with capture_session(ctx.bin, ctx.port, log) as sess:
        time.sleep(1.0)
        sess.stop()
    assert sess.proc.returncode == 0, (
        f"non-zero exit {sess.proc.returncode}\n--- stdout ---\n{sess.output}"
    )
    assert "Decoder:" in sess.output, f"no Decoder line in stdout:\n{sess.output}"
    assert "Stopped. Wrote" in sess.output, (
        f"no clean-stop summary:\n{sess.output}"
    )


def t_ping_pong(ctx) -> None:
    """PING from host, PONG from device — both directions are captured."""
    log = ARTIFACTS / "ping_pong.txt"
    log.unlink(missing_ok=True)
    with capture_session(ctx.bin, ctx.port, log):
        with open_uno(ctx.port) as s:
            s.write(b"PING\n")
            s.flush()
            recv = bytearray()
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline and b"PONG" not in recv:
                recv.extend(s.read(64))
        assert b"PONG" in recv, f"firmware never responded: {bytes(recv)!r}"
        time.sleep(0.5)
    h2d, d2h = parse_log(log.read_text(encoding="utf-8", errors="replace"))
    assert b"PING" in h2d, (
        f"PING not in host->device bytes ({len(h2d)} total): {bytes(h2d)!r}"
    )
    assert b"PONG" in d2h, (
        f"PONG not in device->host bytes ({len(d2h)} total): {bytes(d2h)!r}"
    )


def t_large_payload(ctx) -> None:
    """LARGE: 512 'A' bytes spans many bulk URBs; all should be recovered."""
    log = ARTIFACTS / "large.txt"
    log.unlink(missing_ok=True)
    with capture_session(ctx.bin, ctx.port, log):
        with open_uno(ctx.port) as s:
            s.write(b"LARGE\n")
            s.flush()
            recv = bytearray()
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and recv.count(b"A") < 512:
                recv.extend(s.read(1024))
        assert recv.count(b"A") >= 512, (
            f"firmware sent only {recv.count(b'A')} 'A' bytes"
        )
        time.sleep(1.0)
    _, d2h = parse_log(log.read_text(encoding="utf-8", errors="replace"))
    n = d2h.count(b"A")
    assert n >= 512, f"only {n} 'A' bytes captured (expected >= 512)"


def t_binary_payload(ctx) -> None:
    """BIN 64: 64 non-printable bytes survive intact through the hex column."""
    log = ARTIFACTS / "bin.txt"
    log.unlink(missing_ok=True)
    with capture_session(ctx.bin, ctx.port, log):
        with open_uno(ctx.port) as s:
            s.write(b"BIN 64\n")
            s.flush()
            recv = bytearray()
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and b"END" not in recv:
                recv.extend(s.read(256))
        assert b"END" in recv, "no END marker received from firmware"
        time.sleep(0.5)
    _, d2h = parse_log(log.read_text(encoding="utf-8", errors="replace"))
    expected = bytes((0x80 | (i & 0x7F)) for i in range(64))
    assert expected in d2h, "64-byte binary pattern not found in capture"


def t_printable_only(ctx) -> None:
    """--printable-only filters events with zero printable bytes."""
    log = ARTIFACTS / "printable.txt"
    log.unlink(missing_ok=True)
    with capture_session(ctx.bin, ctx.port, log, "--printable-only"):
        with open_uno(ctx.port) as s:
            s.write(b"BIN 32\n")     # response is pure binary -> filtered
            s.flush()
            time.sleep(0.7)
            s.write(b"PING\n")       # response is text -> kept
            s.flush()
            recv = bytearray()
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline and b"PONG" not in recv:
                recv.extend(s.read(64))
        time.sleep(0.5)
    text = log.read_text(encoding="utf-8", errors="replace")
    h2d, d2h = parse_log(text)
    assert b"PING" in h2d, "host->device 'PING' missing from filtered log"
    assert b"PONG" in d2h, "device->host 'PONG' missing from filtered log"
    binary_blob = bytes((0x80 | (i & 0x7F)) for i in range(32))
    assert binary_blob not in d2h, (
        "--printable-only failed: pure-binary event still in log"
    )


def t_pcap_output(ctx) -> None:
    """--pcap produces a structurally valid pcapng file."""
    log = ARTIFACTS / "pcap.txt"
    pcap = ARTIFACTS / "pcap.pcapng"
    log.unlink(missing_ok=True)
    pcap.unlink(missing_ok=True)
    with capture_session(ctx.bin, ctx.port, log, "--pcap", str(pcap)):
        with open_uno(ctx.port) as s:
            s.write(b"PING\n")
            s.flush()
            time.sleep(1.0)
    assert pcap.exists(), "pcap file was not created"
    assert pcap.stat().st_size > 32, f"pcap suspiciously small: {pcap.stat().st_size} bytes"
    with open(pcap, "rb") as f:
        head = f.read(12)
    # Section Header Block: type=0x0A0D0D0A, total_length (4 bytes), byte-order magic
    assert head[:4] == b"\x0a\x0d\x0d\x0a", (
        f"bad pcapng SHB magic: {head[:4].hex()}"
    )
    bom = head[8:12]
    assert bom in (b"\x4d\x3c\x2b\x1a", b"\x1a\x2b\x3c\x4d"), (
        f"bad byte-order magic: {bom.hex()}"
    )


def t_format_hex(ctx) -> None:
    """--format hex emits hex bytes only, no |...| ASCII column."""
    log = ARTIFACTS / "fmt_hex.txt"
    log.unlink(missing_ok=True)
    with capture_session(ctx.bin, ctx.port, log, "--format", "hex"):
        with open_uno(ctx.port) as s:
            s.write(b"PING\n")
            s.flush()
            time.sleep(1.0)
    text = log.read_text(encoding="utf-8", errors="replace")
    assert "50 49 4e 47" in text.lower(), "PING bytes not visible as hex"
    assert not re.search(r"\|[^|\n]+\|", text), (
        "ASCII column present despite --format hex"
    )


def t_format_ascii(ctx) -> None:
    """--format ascii emits the ASCII column only."""
    log = ARTIFACTS / "fmt_ascii.txt"
    log.unlink(missing_ok=True)
    with capture_session(ctx.bin, ctx.port, log, "--format", "ascii"):
        with open_uno(ctx.port) as s:
            s.write(b"PING\n")
            s.flush()
            time.sleep(1.0)
    text = log.read_text(encoding="utf-8", errors="replace")
    assert "PING" in text, "PING text not present in ASCII-format log"
    assert not re.search(r"(?:[0-9a-f]{2} ){4}", text.lower()), (
        "hex bytes column present despite --format ascii"
    )


TESTS = [
    ("passive_smoke",   t_passive_smoke),
    ("ping_pong",       t_ping_pong),
    ("large_payload",   t_large_payload),
    ("binary_payload",  t_binary_payload),
    ("printable_only",  t_printable_only),
    ("pcap_output",     t_pcap_output),
    ("format_hex",      t_format_hex),
    ("format_ascii",    t_format_ascii),
]


# ---------------------------------------------------------------------------
# driver

class Ctx:
    def __init__(self, bin_path: Path, port: str):
        self.bin = bin_path
        self.port = port


def _is_admin() -> bool:
    """True if the current process is elevated. Windows-only; returns True on
    other platforms so callers can guard with `os.name == "nt"`."""
    if os.name != "nt":
        return True
    try:
        import ctypes
        return bool(ctypes.windll.shell32.IsUserAnAdmin())
    except Exception:
        return False


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument(
        "--bin", default=str(DEFAULT_BIN),
        help=f"path to serial-capture binary (default: {DEFAULT_BIN})",
    )
    ap.add_argument("--port", default=None, help="COM port (auto-detected if omitted)")
    ap.add_argument(
        "--list", action="store_true",
        help="list visible serial ports with VID:PID and exit",
    )
    ap.add_argument(
        "--only", action="append", default=[], metavar="NAME",
        help="run only the named test (repeatable)",
    )
    args = ap.parse_args()

    if args.list:
        for p in serial.tools.list_ports.comports():
            vid = f"{p.vid:04x}" if p.vid is not None else "----"
            pid = f"{p.pid:04x}" if p.pid is not None else "----"
            print(f"{p.device}\t{vid}:{pid}\t{p.description}")
        return 0

    if os.name == "nt" and not _is_admin():
        print(
            "tests must run from an elevated shell — USBPcap's ACL on \\\\.\\USBPcapN\n"
            "is Administrators-only, so the binary itself needs admin to capture.\n"
            'Right-click PowerShell or Windows Terminal → "Run as administrator",\n'
            "then re-run this command.",
            file=sys.stderr,
        )
        return 1

    bin_path = Path(args.bin)
    if not bin_path.exists():
        print(
            f"binary not found: {bin_path}\n"
            f"build it from the repo root with: cargo build --release",
            file=sys.stderr,
        )
        return 2

    port = args.port or autodetect_port()
    if not port:
        print(
            "no Arduino-like device detected. List ports with --list, "
            "or pass --port COMx",
            file=sys.stderr,
        )
        return 2

    print(f"binary:   {bin_path}")
    print(f"port:     {port}")

    try:
        banner = verify_firmware(port)
    except Exception as e:
        print(f"could not talk to firmware on {port}: {e}", file=sys.stderr)
        return 2
    if "READY:serial-capture-test" not in banner:
        print(
            f"unexpected firmware banner: {banner!r}\n"
            f"did you flash tests/uno_firmware/uno_firmware.ino?",
            file=sys.stderr,
        )
        return 2
    print(f"firmware: {banner}")

    ARTIFACTS.mkdir(exist_ok=True)
    selected = (
        TESTS if not args.only
        else [(n, fn) for n, fn in TESTS if n in args.only]
    )
    if not selected:
        print(f"no tests match --only {args.only}", file=sys.stderr)
        return 2

    ctx = Ctx(bin_path, port)
    failures: list[tuple[str, str]] = []
    print()
    for name, fn in selected:
        print(f"[{name:>16}] ", end="", flush=True)
        t0 = time.monotonic()
        try:
            fn(ctx)
            print(f"ok    ({time.monotonic() - t0:5.1f}s)")
        except AssertionError as e:
            failures.append((name, str(e)))
            print(f"FAIL  ({time.monotonic() - t0:5.1f}s)")
            for line in str(e).splitlines():
                print(f"    {line}")
        except Exception as e:
            failures.append((name, repr(e)))
            print(f"ERROR ({time.monotonic() - t0:5.1f}s)")
            print(f"    {e!r}")

    print()
    print(f"{len(selected) - len(failures)}/{len(selected)} passed")
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
