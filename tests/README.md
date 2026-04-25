# Test suite

End-to-end tests for `serial-capture` against an Arduino Uno on Windows.
Exists primarily to confirm passive-mode capture works after the USBPcap
IOCTL fixes (see `b0468ce`, `4fbb3b3`, `5a87988`).

## What it covers

Passive mode (USBPcap):

| Test            | Verifies                                                        |
|-----------------|-----------------------------------------------------------------|
| passive_smoke   | capture starts, exits cleanly on Ctrl-Break, prints summary     |
| ping_pong       | bidirectional capture: host->device and device->host            |
| large_payload   | 512-byte response spans many bulk URBs, all bytes recovered     |
| binary_payload  | non-printable bytes (0x80-0xFF) round-trip via the hex column   |
| printable_only  | `--printable-only` filters events with zero printable bytes     |
| pcap_output     | `--pcap` produces a structurally valid pcapng file              |
| format_hex      | `--format hex` emits hex only, no ASCII column                  |
| format_ascii    | `--format ascii` emits ASCII only, no hex column                |

Out of scope: active mode (`--active`), multi-controller `\\.\USBPcapN`
selection, FTDI/CH340/PL2303 framing decoders (covered by Rust unit tests).

## Setup

1. **Flash the firmware.**
   - Open `tests/uno_firmware/uno_firmware.ino` in the Arduino IDE.
   - Tools -> Board -> Arduino Uno; Tools -> Port -> the Uno's COM port.
   - Upload. The firmware runs at 115200 baud and prints
     `READY:serial-capture-test:v1` on startup.

2. **Install USBPcap 1.5.4.0.** Either:
   - Run `serial-capture --port COMx -o cap.txt` once and accept the UAC
     prompt, then unplug and replug the Uno; **or**
   - Install manually from <https://desowin.org/usbpcap/>.

3. **Build serial-capture** from the repo root:

   ```powershell
   cargo build --release
   ```

4. **Install Python dependencies:**

   ```powershell
   pip install pyserial
   ```

   Python 3.8+ is required.

## Run

From the repo root:

```powershell
python tests\run_tests.py
```

The harness auto-detects the Uno by VID. To override:

```powershell
python tests\run_tests.py --port COM4
python tests\run_tests.py --bin path\to\serial-capture.exe
python tests\run_tests.py --list                 # show all serial ports
python tests\run_tests.py --only ping_pong       # run a single test
python tests\run_tests.py --only ping_pong --only large_payload
```

Exit code is `0` on success and `1` on any failure. Captured logs and
pcapng files are kept under `tests/_artifacts/` for inspection.

## Troubleshooting

- **"no Arduino-like device detected"** — pass `--port COMx` explicitly,
  or check `--list`. Add the VID to `KNOWN_VIDS` in `run_tests.py` if your
  clone uses an unusual chip.
- **"unexpected firmware banner"** — re-flash `uno_firmware/uno_firmware.ino`.
- **"capture did not start within timeout"** — usually USBPcap isn't
  installed, or the Uno was plugged in before USBPcap's filter driver
  loaded. Replug and retry. The harness prints the captured stdout so the
  underlying error is visible.
- **`large_payload` reports fewer than 512 'A' bytes** — this would be
  a real capture-loss bug worth investigating; USBPcap should not drop
  full-speed bulk URBs at this rate. Check `tests/_artifacts/large.txt`
  and consider grabbing a `--pcap` to compare against Wireshark.
