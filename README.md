# serial-capture

Cross-platform USB virtual COM port capture for Linux and Windows. Records
traffic to a timestamped text log and (optionally) a Wireshark-readable pcapng.

Supported chips: **CDC-ACM**, **FTDI** (FT232/FT2232/FT4232/FT-X),
**WCH CH340/341/343/9102**, **Prolific PL2303**, **Silicon Labs CP210x**
(CP2102/CP2102N/CP2103/CP2104/CP2105/CP2108/CP2109).

```
serial-capture                                              # auto-detect, → stdout
serial-capture -o capture.txt                               # auto-detect, → file
serial-capture --port /dev/ttyACM0 -o capture.txt           # explicit port
serial-capture --port COM4 -o capture.txt --pcap capture.pcapng
```

`--port` is optional: when exactly one USB serial device is connected, the
tool picks it. Otherwise it lists the candidates and exits.

## Install

```
cargo install serial-capture
```

`cargo install` is the only supported distribution today. The pinned USBPcap
installer (~190 KB) ships in `vendor/` and is embedded into the Windows binary
at build time — no network access required. Override with
`USBPCAP_INSTALLER=/path/to/file cargo build` if you need a different version.

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

## Testing for data loss

Serial links fail silently: the UART, the USB-UART chip, and USBPcap's kernel
ring buffer all drop data without telling anyone. On Windows the ring buffer
is the usual suspect during bursts — when it fills, the USBPcap driver throws
whole packets away. Two things make that visible:

- `--usbpcap-buffer-mb <MIB>` (default 16, max 128) sizes that ring buffer.
  The driver stores USB packets, not serial bytes: at 921600 baud a PL2303
  delivers one-byte URBs, which cost ~44 bytes each in the ring (~4 MB/s of
  traffic, so 16 MiB is ~4 seconds of slack). An FTDI sends 64-byte URBs and
  gets minutes out of the same buffer. The chosen size is printed at startup.
- The reader drains the ring in 256 KiB `ReadFile` calls, so one syscall
  covers hundreds of queued URBs instead of two per packet. Without that, a
  one-byte-per-URB driver at high baud keeps the userspace side busy enough to
  let the kernel buffer overflow.
- When capture stops, the byte totals are printed. Compare them with what the
  sender reports:

  ```
  Stopped. Wrote 5000 line(s).
  → capture: 51234 packet(s), 9843 for this device; IN 9843 URB(s) / 250000 B → 250000 B decoded; OUT 0 URB(s) / 0 B → 0 B decoded
  ```

  `IN 250000 B` matching the sender's byte count means nothing was lost on
  the capture side. `decoded` below the raw count means the decoder stripped
  bytes (modem status), which is expected for some chips.

`tools/` has two PowerShell scripts for an end-to-end check on Windows:

```powershell
# Terminal 1 (elevated): capture the sending port.
.\serial-capture.exe --port COM6 --format ascii --wrap 0 -o .\run.log

# Terminal 2: 5000 numbered lines, 50 bytes each, as fast as the UART allows.
.\tools\throughput-test.ps1 -Port COM6 -Baud 921600 -Lines 5000 -PayloadBytes 50

# Terminal 1: Ctrl-C, then check for gaps.
.\tools\verify-capture.ps1 -Path .\run.log -Expected 5000
```

Capturing the *receiving* port is the harsher test for PL2303, whose Windows
driver reads one byte per URB. Nothing is read from that endpoint unless an
application has the port open, so hold it open with `receive-test.ps1` — that
also programs the chip's baud rate:

```powershell
# Terminal 1 (elevated): capture COM7.
.\serial-capture.exe --port COM7 --format ascii --wrap 0 -o .\cap.log

# Terminal 2: keep COM7 open at the same baud as the sender.
.\tools\receive-test.ps1 -Port COM7 -Baud 921600 -Seconds 30 -OutFile .\rx.log
```

Then compare three numbers: sender bytes (printed by the sender) vs
`rx.log` length (UART/chip loss) vs the capture's `IN … B` total
(USBPcap/capture-process loss).

Capture is passive, so it sees traffic while another program owns the port.
For the harshest test, capture the PL2303 side: its Windows driver issues one
URB per byte, so 921600 baud means ~92,000 URBs per second.

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
re-enumerates). Then re-run **from an elevated shell** — USBPcap's
installer sets an Administrators-only ACL on `\\.\USBPcapN`, so the
capture process itself must be elevated. The tool detects non-elevated
runs and prints instructions.

**Port resolution.** Present ports are enumerated through SetupAPI and then
walked up the device tree to the underlying USB device. This covers drivers
that enumerate outside `Enum\USB` (notably FTDI's `FTDIBUS` enumerator) and
composite CDC-ACM interfaces, and it ignores unplugged devices that linger in
the registry as "ghost" COM ports.

The USB address used for capture comes from the parent hub
(`IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX`), not from the registry. The
`Address` value under `Enum\USB\...` is written at install time and can be
stale — a PL2303 in hub port 3 reported `Address` 3 while its live address was
10, so filtering by the registry value captured a different device (or
nothing).

**Multi-controller systems.** USBPcap installs one `\\.\USBPcapN` interface
per USB host controller. The tool selects the interface attached to the
device's root hub, and stops with instructions if it cannot pick one.
Override with `--usbpcap '\\.\USBPcap2'` (or 3, 4, …).

**When nothing appears.** Two things are worth checking first, and neither
needs Wireshark:

1. `--list-usbpcap` prints every present port with its USB parent, address and
   root hub, plus each `USBPcapN` and the hub it is attached to. It runs
   without Administrator. Compare the normalized root hubs to see which
   interface the device is on.
2. `--debug-usb` ignores the address filter and prints one line per USB packet
   on the chosen interface: `[usb] dev 5 ep 0x81 bulk len 64 …`. Generate
   serial traffic, and the line shows the real device address. If the address
   differs from the one Windows reports, pass `--usb-address <N>`.

Writing to the port under capture is enough to produce traffic — no second
board needed. In a second window:

```powershell
$p = [System.IO.Ports.SerialPort]::new('COM6', 115200)
$p.Open(); 1..20 | ForEach-Object { $p.WriteLine("ping $_") ; Start-Sleep -m 200 }
$p.Close()
```

## CLI reference

```
serial-capture [--port <PORT>] [options]

Port selection
  --port <PORT>        COM4 (Windows) or /dev/ttyUSB0 / /dev/ttyACM0 (Linux).
                       Optional: if there's exactly one USB serial device on
                       the system, it's picked automatically.

Output
  -o, --output <FILE>  Text log path. Omit to write events to stdout.
  --pcap <FILE>        Also write Wireshark-compatible pcapng
  --format <hex|ascii|both>   Text log columns (default: both)
  --printable-only     Skip events with zero text bytes
  -q, --quiet          Suppress live tail to stdout (with -o), or suppress
                       all text output (without -o; useful with --pcap)
  --no-assemble        Print one row per captured URB. By default consecutive
                       URBs are joined and split on newlines, so a line stays
                       one row even when the driver delivers it byte by byte
                       (PL2303 on Windows does this).
  --wrap <BYTES>       Bytes per output row (default 16). Continuation rows of
                       a long line are indented instead of repeating the
                       timestamp. Use 0 to print each line as a single row.

First-run / privileges
  -y, --yes            Linux: auto-confirm sudo prompts (load usbmon, write
                       udev rule). No effect when setup is already complete.

Windows
  --usbpcap <NAME>     Pick a specific filter interface, e.g.
                       '\\.\USBPcap2'. Defaults to the interface attached to
                       the device's root hub.
  --list-usbpcap       Print present ports, root hubs and every USBPcap
                       interface, then exit. Works without Administrator.
  --debug-usb          Capture all devices and print one line per USB packet.
                       Use it to find the real device address.
  --usb-address <N>    Override the USB device address used for filtering.
  --usbpcap-buffer-mb <MIB>
                       USBPcap kernel ring-buffer size, 4-128 (default 16).
                       Bigger survives longer reader stalls; the driver drops
                       packets silently when it fills.

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

Per-chip decoders live in `src/decode/`. The CDC-ACM, CH340, PL2303, and
CP210x decoders are passthroughs (raw bytes on bulk IN/OUT). The FTDI decoder
strips the 2-byte modem-status header that FTDI prepends to every
`wMaxPacketSize` chunk on its bulk-IN endpoint.

Text output is line-oriented: bytes from consecutive URBs in the same
direction are joined, split on `\n`, and flushed when the direction changes or
the stream stalls for 50 ms. This matters on Windows, where the PL2303 driver
reads one byte at a time — without it a single line would print as one row per
character. A line longer than `--wrap` bytes (default 16) continues on indented
rows without repeating the timestamp, and `--wrap 0` prints it as one row.
`--no-assemble` restores per-URB rows; `--pcap` always stores the raw URBs.

## Limitations / future work

- Hub-IOCTL endpoint discovery on Windows (current FTDI MPS is a PID-based
  heuristic; `--ftdi-mps` overrides)
- Multi-UART CP2105/CP2108 on Windows: endpoint discovery is not implemented,
  so traffic from all UARTs of one chip is merged into a single log
- Tested chip families: CDC-ACM, FTDI logic verified by unit tests; live
  end-to-end testing requires the real hardware

## License

GPL-3.0-or-later. See [LICENSE](LICENSE) for the full text.

## Acknowledgements

- [USBPcap](https://desowin.org/usbpcap/) — Windows USB packet capture driver.
  The unmodified upstream installer (`vendor/USBPcapSetup-1.5.4.0.exe`) is
  bundled and run as a separate process; the installer's components carry
  their own licenses (USBPcapDriver: GPL-2.0; USBPcapCMD: BSD-2-Clause).
- Linux `usbmon` and the libpcap project
