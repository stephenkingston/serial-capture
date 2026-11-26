#!/usr/bin/env python3
"""Check a serial-capture log for missing SEQ numbers.

A gap means bytes were lost between the sender and the log: the UART, the
USB-UART chip, USBPcap's ring buffer, or the capture process.

Use ASCII or Both output format (Both is the default); with Both the token is
in the ASCII column. Hex-only logs cannot be checked this way.
"""

import argparse
import re
import sys
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("path", type=Path)
    ap.add_argument(
        "--pattern",
        default=r"SEQ(\d{6})",
        help=r"regex with one capture group (default 'SEQ(\d{6})')",
    )
    ap.add_argument(
        "-e",
        "--expected",
        type=int,
        default=0,
        help="expected line count; checks 0..N-1 (0 = infer from the highest seen)",
    )
    ap.add_argument(
        "--max-ranges", type=int, default=20, help="missing ranges to print"
    )
    a = ap.parse_args()

    if not a.path.is_file():
        print(f"error: no such file: {a.path}", file=sys.stderr)
        return 2

    rx = re.compile(a.pattern)
    seen: set[int] = set()
    tokens = 0
    bad = 0

    with a.path.open("rb") as f:
        for raw in f:
            for m in rx.finditer(raw.decode("utf-8", "replace")):
                tokens += 1
                try:
                    seen.add(int(m.group(1)))
                except ValueError:
                    bad += 1

    if not seen:
        print(f"No '{a.pattern}' tokens found in {a.path}.", file=sys.stderr)
        return 2

    lo = 0 if a.expected > 0 else min(seen)
    hi = a.expected - 1 if a.expected > 0 else max(seen)

    ranges: list[tuple[int, int]] = []
    missing = 0
    start = None
    for n in range(lo, hi + 1):
        if n not in seen:
            if start is None:
                start = n
            missing += 1
        elif start is not None:
            ranges.append((start, n - 1))
            start = None
    if start is not None:
        ranges.append((start, hi))

    print(f"File:              {a.path}")
    print(f"Tokens found:      {tokens} ({len(seen)} distinct)")
    print(f"Observed range:    {min(seen)}..{max(seen)}")
    print(f"Expected range:    {lo}..{hi} ({hi - lo + 1} line(s))")
    if bad:
        print(f"Unparsable tokens: {bad}")

    if missing == 0:
        print("Result:            OK - no gaps.")
        return 0

    print(f"Result:            {missing} line(s) MISSING")
    print(f"Missing ranges:    {len(ranges)}")
    for first, last in ranges[: a.max_ranges]:
        print(f"  missing {first}" if first == last else f"  missing {first}-{last}")
    if len(ranges) > a.max_ranges:
        print(f"  ... and {len(ranges) - a.max_ranges} more range(s)")
    return 1


if __name__ == "__main__":
    sys.exit(main())
