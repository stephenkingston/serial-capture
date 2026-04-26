# serial-capture

Cross-platform USB virtual COM port capture for Linux and Windows. Records
traffic to a timestamped text log and (optionally) a Wireshark-readable pcapng.

Supported chips: **CDC-ACM**, **FTDI** (FT232/FT2232/FT4232/FT-X),
**WCH CH340/341/343/9102**, **Prolific PL2303**.

```
serial-capture --port /dev/ttyACM0                          # → stdout
serial-capture --port /dev/ttyACM0 -o capture.txt
serial-capture --port COM4         -o capture.txt --pcap capture.pcapng
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

## How it works

```
serial-capture --port COM4 -o capture.txt
```

Captures at the USB layer using **USBPcap** on Windows (auto-installed on
first run via UAC) and **usbmon** on Linux. The user's application keeps
talking to the real port unchanged; we sniff URBs and decode chip-specific
framing (FTDI's 2-byte status header, etc.) to recover serial bytes.

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
get control transfers, descriptor exchanges, and bulk traffic.

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
  /etc/udev/rules.d/60-serial-capture.rules. Persists across reboots
  and replugs.
→ NOTE: any local user on this machine will then be able to sniff
  USB traffic — keystrokes, USB drives, smartcards, anything on the
  bus. Fine for a single-user dev box; on a shared system, decline
  and run serial-capture with sudo instead.
Install udev rule and chmod /dev/usbmon* (one sudo invocation)? [Y/n]
```

**Security note.** The auto-setup makes `/dev/usbmon*` world-readable
because that's the only way to grant persistent access without a
logout/login dance (process supplementary groups are frozen at login,
so a fresh `serialcap` group can't apply to your current shell). For a
single-user dev box, the practical risk is approximately zero. On a
shared system — decline the prompt and run `sudo serial-capture …`
instead, or write your own group-scoped rule before re-running.

To also avoid loading the module by hand after a reboot:

```
echo usbmon | sudo tee /etc/modules-load.d/usbmon.conf
```

### Windows passive

USBPcap is auto-installed on first run via UAC. After the install, **unplug
and replug the target device** (USBPcap's filter driver attaches at PnP
enumeration; existing connections aren't filtered until the device
re-enumerates). Then re-run.

## CLI reference

```
serial-capture --port <PORT> [options]

Required
  --port <PORT>        COM4 (Windows) or /dev/ttyUSB0 / /dev/ttyACM0 (Linux)

Output
  -o, --output <FILE>  Text log path. Omit to write events to stdout.
  --pcap <FILE>        Also write Wireshark-compatible pcapng
  --format <hex|ascii|both>   Text log columns (default: both)
  --printable-only     Skip events with zero text bytes
  -q, --quiet          Suppress live tail to stdout (with -o), or suppress
                       all text output (without -o; useful with --pcap)

Decoder tuning
  --ftdi-mps <BYTES>   Force FTDI bulk-IN wMaxPacketSize (64 or 512). Use only
                       when the auto-detection picks the wrong size for an
                       unusual variant or clone.
```

## Architecture

```
              Linux:    /dev/usbmonN  ──libpcap──▶  decoder  ──▶  text + pcapng
User app  ──▶ Windows:  \\.\USBPcap1  ──Win32───▶  decoder  ──▶  text + pcapng
```

Per-chip decoders live in `src/decode/`. The CDC-ACM, CH340, and PL2303
decoders are passthroughs (raw bytes on bulk IN/OUT). The FTDI decoder strips
the 2-byte modem-status header that FTDI prepends to every `wMaxPacketSize`
chunk on its bulk-IN endpoint.

## Limitations / future work

- Hub-IOCTL endpoint discovery on Windows (current FTDI MPS is a PID-based
  heuristic; `--ftdi-mps` overrides)
- Multi-controller `\\.\USBPcapN` selection (currently hardcoded to USBPcap1)
- Tested chip families: CDC-ACM, FTDI logic verified by unit tests; live
  end-to-end testing requires the real hardware

## License

MIT OR Apache-2.0.

## Acknowledgements

- [USBPcap](https://desowin.org/usbpcap/) — Windows USB packet capture driver
- Linux `usbmon` and the libpcap project
