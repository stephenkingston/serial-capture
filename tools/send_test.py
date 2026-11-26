#!/usr/bin/env python3
"""Send SEQ-numbered lines to a serial port as fast as it accepts them.

Every line is ``SEQ<6 digits>,<padding>\\r\\n``, so the expected byte count is
exact and a capture log can be checked for gaps with verify_capture.py.

Writes block when the UART or the driver queue is full, so this script cannot
silently lose data: if the capture saw fewer bytes, the loss happened after
the host handed them to the kernel (USBPcap ring buffer, USB-UART chip, or the
capture process).

Works on Linux/WSL and Windows (needs pyserial: ``pip install pyserial``).
"""

import argparse
import sys
import time

import serial


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("port", help="e.g. /dev/ttyUSB0 or COM6")
    ap.add_argument("-b", "--baud", type=int, default=115200)
    ap.add_argument("-n", "--lines", type=int, default=5000)
    ap.add_argument(
        "-p",
        "--payload-bytes",
        type=int,
        default=50,
        help="payload per line including 'SEQ000000,' (default 50)",
    )
    ap.add_argument(
        "-d",
        "--delay-ms",
        type=int,
        default=0,
        help="sleep after each 128-line batch (default 0 = full speed)",
    )
    ap.add_argument(
        "--no-handshake",
        action="store_true",
        help="leave DTR/RTS alone (some boards gate the UART on them)",
    )
    a = ap.parse_args()

    if a.payload_bytes < 10:
        print("error: --payload-bytes must be at least 10 (SEQ000000,)", file=sys.stderr)
        return 2
    if a.lines < 1:
        print("error: --lines must be at least 1", file=sys.stderr)
        return 2

    padding = "x" * (a.payload_bytes - 10)
    total_bytes = a.lines * (a.payload_bytes + 2)

    with serial.Serial(a.port, a.baud, timeout=0, write_timeout=30) as s:
        if not a.no_handshake:
            s.dtr = True
            s.rts = True

        start = time.perf_counter()
        batch = []
        for i in range(a.lines):
            batch.append(f"SEQ{i:06d},{padding}\r\n")
            if len(batch) >= 128 or i == a.lines - 1:
                s.write("".join(batch).encode("ascii"))
                s.flush()
                batch.clear()
                if a.delay_ms:
                    time.sleep(a.delay_ms / 1000)
        elapsed = max(time.perf_counter() - start, 1e-9)

    print(f"Sent {a.lines} line(s), {total_bytes} byte(s) in {elapsed:.2f} s "
          f"({total_bytes / elapsed / 1024:.1f} KiB/s).")
    print(f"Expected capture: {a.lines} line(s), {total_bytes} byte(s).")
    print(f"First line: SEQ000000,...   Last line: SEQ{a.lines - 1:06d},...")
    return 0


if __name__ == "__main__":
    sys.exit(main())
