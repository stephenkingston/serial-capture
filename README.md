# serial-capture

Cross-platform USB virtual COM port capture for Linux and Windows. Records
traffic to a timestamped text log and (optionally) a Wireshark-readable pcapng.

Supported chips: **CDC-ACM**, **FTDI** (FT232/FT2232/FT4232/FT-X),
**WCH CH340/341/343/9102**, **Prolific PL2303**.

```
serial-capture --port /dev/ttyACM0 -o capture.txt
serial-capture --port COM4         -o capture.txt --pcap capture.pcapng
serial-capture --port COM4         -o capture.txt --active --baud 115200
```

## Install

```
cargo install serial-capture
```

`cargo install` is the only supported distribution today. Builds run a small
`build.rs` that fetches the pinned USBPcap installer (~190 KB) from GitHub and
embeds it into the Windows binary; set `USBPCAP_INSTALLER=/path/to/file` to
build offline.

Platform support:

| Target              | Status                                    |
|---------------------|-------------------------------------------|
| Linux x86_64        | supported                                  |
| Linux aarch64       | supported (passive only; needs usbmon)     |
| Windows x86_64      | supported                                  |
| Windows ARM64       | not supported (no signed USBPcap driver)   |
| macOS               | not supported                              |

The unsupported platforms fail at compile time with a clear `compile_error!`
and at runtime with an explanatory message, so misuse is hard.

## Two modes

### Passive (default) — captures USB URBs

```
serial-capture --port COM4 -o capture.txt
```

Uses **USBPcap** on Windows (auto-installed on first run via UAC) and
**usbmon** on Linux. The user's application keeps talking to the real port
unchanged; we sniff at the USB layer and decode chip-specific framing
(FTDI's 2-byte status header, etc.) to recover serial bytes.

### Active (`--active`) — proxy through a virtual port pair

```
serial-capture --port /dev/ttyUSB0 -o capture.txt --active --baud 115200
```

Creates a pty (Linux) or uses **com0com** (Windows). The tool opens the real
port itself and exposes a virtual endpoint for the user's application; bytes
flow through us so we can log them, no USB sniffing required. Useful when:

- Passive capture is unavailable (e.g. Windows ARM64 — well, that's blocked
  entirely; or a multi-controller box where bus mapping is unsolved)
- You want byte-perfect logs without parsing chip framing
- You only have user-mode access (active mode needs no kernel driver on Linux)

Active mode requires the user to point their application at the printed
virtual endpoint instead of the real port.

## Output

Default text log format (`--format both`, the default):

```
2026-04-25 12:30:45.123  →  68 65 6c 6c 6f 0d 0a                            |hello..|
2026-04-25 12:30:45.140  ←  4f 4b 0d 0a                                     |OK..|
```

Arrows: `→` = host application → device, `←` = device → host application.

Other formats: `--format hex`, `--format ascii`. Live tail to stdout unless
`--quiet`. `--printable-only` skips events whose payload contains zero text
bytes (printable ASCII or `\t` `\n` `\r`) — useful for filtering binary
keep-alive chatter from a text protocol.

`--pcap FILE` additionally writes a Wireshark-compatible pcapng of the
device's full URB stream (link types `DLT_USB_LINUX_MMAPPED` on Linux,
`DLT_USBPCAP` on Windows). Wireshark's USB dissector reads it natively; you
get control transfers, descriptor exchanges, and bulk traffic. Not available
in `--active` mode (no URB stream exists at the proxy layer).

## First-run requirements

### Linux passive

`usbmon` is in-tree. On first run, if it isn't loaded or `/dev/usbmonN`
isn't readable, the tool offers to fix it for you — one prompt before
each `sudo` invocation, or pass `--yes` / `-y` to skip the prompts:

```
→ /dev/usbmon* is missing. The usbmon kernel module is not loaded.
Run 'sudo modprobe usbmon'? [Y/n]
→ /dev/usbmon* is not readable by the current user.
→ Installing a permissive udev rule (mode 0644 / world-readable) at
  /etc/udev/rules.d/60-serial-capture.rules so /dev/usbmon* stays
  accessible across reboots and replugs.
Install udev rule and chmod /dev/usbmon* (one sudo invocation)? [Y/n]
```

The udev rule is **world-readable** — anyone on the box can sniff USB.
Fine for a dev machine. The rule persists across reboots and module
reloads, so this only happens once.

To also avoid loading the module by hand after a reboot:

```
echo usbmon | sudo tee /etc/modules-load.d/usbmon.conf
```

### Windows passive

USBPcap is auto-installed on first run via UAC. After the install, **unplug
and replug the target device** (USBPcap's filter driver attaches at PnP
enumeration; existing connections aren't filtered until the device
re-enumerates). Then re-run.

### Windows active

`com0com` is required and must currently be installed manually. Auto-install
is not yet wired. The tool prints exact instructions on first run:

1. Download Pete Batard's signed 2.2.2.0 build:
   <https://files.akeo.ie/blog/com0com.7z>
2. Extract and install (Administrator command prompt):
   ```
   pnputil /add-driver x64\com0com.inf /install
   setupc.exe install
   ```
3. Re-run with `--active`.

## CLI reference

```
serial-capture --port <PORT> -o <FILE> [options]

Required
  --port <PORT>        COM4 (Windows) or /dev/ttyUSB0 / /dev/ttyACM0 (Linux)
  -o, --output <FILE>  Text log path

Mode
  --active             Proxy via pty (Linux) or com0com (Windows) instead of
                       passive USB sniffing. Requires reconfiguring the user
                       application to the printed virtual endpoint.
  --baud <BAUD>        Real-port baud in --active mode (default: 9600)

Output
  --pcap <FILE>        Also write Wireshark-compatible pcapng (passive only)
  --format <hex|ascii|both>   Text log columns (default: both)
  --printable-only     Skip events with zero text bytes
  -q, --quiet          Suppress live tail to stdout

Decoder tuning
  --ftdi-mps <BYTES>   Force FTDI bulk-IN wMaxPacketSize (64 or 512). Use only
                       when the auto-detection picks the wrong size for an
                       unusual variant or clone.
```

## Architecture

```
            ┌──────────────────────────── passive (default) ────────────────────────────┐
            │                                                                           │
            │   Linux:    /dev/usbmonN  ──libpcap──▶  decoder  ──▶  text + pcapng       │
User app  ──┤   Windows:  \\.\USBPcap1  ──Win32───▶  decoder  ──▶  text + pcapng       │
   │        └───────────────────────────────────────────────────────────────────────────┘
   │
   │        ┌──────────────────────────── active (--active) ────────────────────────────┐
   ▼        │                                                                           │
[real port] │   Linux:    pty ⇄ proxy ⇄ /dev/ttyUSB0   ──tee──▶  text                  │
            │   Windows:  CNCB0 ⇄ proxy ⇄ COM4         ──tee──▶  text                  │
            └───────────────────────────────────────────────────────────────────────────┘
```

Per-chip decoders live in `src/decode/`. The CDC-ACM, CH340, and PL2303
decoders are passthroughs (raw bytes on bulk IN/OUT). The FTDI decoder strips
the 2-byte modem-status header that FTDI prepends to every `wMaxPacketSize`
chunk on its bulk-IN endpoint.

## Limitations / future work

- Hub-IOCTL endpoint discovery on Windows (current FTDI MPS is a PID-based
  heuristic; `--ftdi-mps` overrides)
- com0com auto-install on Windows
- Serial state mirroring in active mode (DTR/RTS/parity/break)
- Multi-controller `\\.\USBPcapN` selection (currently hardcoded to USBPcap1)
- Tested chip families: CDC-ACM, FTDI logic verified by unit tests; live
  end-to-end testing requires the real hardware

## License

MIT OR Apache-2.0.

## Acknowledgements

- [USBPcap](https://desowin.org/usbpcap/) — Windows USB packet capture driver
- [com0com](https://com0com.sourceforge.net/) — Windows null-modem emulator
- [Pete Batard's signed com0com 2.2.2.0 build](https://pete.akeo.ie/2011/07/com0com-signed-drivers.html)
- Linux `usbmon` and the libpcap project
