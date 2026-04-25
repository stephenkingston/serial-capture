# serial-capture — design + build conversation

This file is an export of the conversation that designed and built this tool, from the initial requirements through the live USBPcap bug fixes. Tool outputs and intermediate iterations are summarized where they didn't carry information that mattered downstream.

---

## 1 — Initial request

**User:** I want to build USB traffic capture software for use in Windows and Linux, especially capturing Virtual COM port traffic (e.g. CH340, FT232, FT2232, PL2303). Capture traffic in a text file (option to ignore other traffic). `serial-capture --port COM4 -o capture.txt`. Use USBPcap driver. Use Rust. Plan this task. Ask questions when in doubt. UX should be frictionless. On launch, check for USBPcap; if not found, install automatically.

**Assistant:** Before locking the plan, flagging things I'm uncertain about:

1. **USBPcap is Windows-only.** No Linux build. Linux equivalent is the in-tree `usbmon` module, accessible via libpcap. So "use USBPcap" naturally means **USBPcap on Windows + usbmon on Linux**, sharing a common pcap-based capture core.
2. **Capture is at the USB URB layer.** USBPcap/usbmon hand you raw URBs; producing clean serial bytes for CH340/FTDI/PL2303 needs per-chip wrapper decoding (FTDI prepends 2 status bytes per packet on bulk IN; CH340/PL2303 differ).
3. **`COM4` → USB device mapping is non-trivial on Windows** — walk SetupAPI/registry. Linux equivalent is sysfs.
4. **Auto-installing USBPcap requires UAC elevation.**

Then I asked clarifying questions:

1. Capture model — passive (sniff) or active (proxy via virtual COM pair)? I assumed passive.
2. Text format — timestamped hex+ASCII with direction tags? Configurable?
3. Also emit `.pcapng` for Wireshark?
4. What does "ignore other traffic" mean exactly? My read: skip control transfers, keep bulk payload — confirm.
5. Chip scope — just CH340/FT232/FT2232/PL2303, or include CDC-ACM (Arduino, etc.) too?
6. Live tail to stdout?
7. Linux privilege handling — single sudo prompt that installs a udev rule + group membership, or sudo every time?
8. Distribution — `cargo install`, prebuilt binary, MSI/.deb?
9. Multi-port capture in v1?

---

## 2 — User answers (round 1)

**User:** 1. active 2. timestamped + Hex + ASCII 3. yes but behind a CLI flag 4. yes behind a CLI flag 5. Add CDC-ACM too 6. yes 7. yes 8. cargo install (only) 9. single port

**Assistant:** Two things ripple back into the design:

**A.** Active proxy means we don't need USBPcap at all in the default flow. Auto-install target shifts from **USBPcap → com0com** (virtual COM-pair driver) on Windows. Linux uses kernel pty (no install). USBPcap re-enters only as an optional `--passive` mode for users who can't reconfigure their app.

**B.** "Ignore other traffic" flag in active mode: there is no other traffic at the proxy layer — we only see bytes the app pushes through. The flag only becomes meaningful as a text-output filter, e.g. `--printable-only`. Confirm intent or we drop it.

I sketched the proposed plan: active proxy by default, `--passive` as opt-in, com0com auto-install on Windows, pty on Linux, com0com 2.2.2.0 (Pete Batard's signed build) pinned for Win11 compatibility. CLI: `--port`, `-o`, `--baud`, `--pcap`, `--printable-only`, `--quiet`, `--format hex|ascii|both`. Module layout, frictionless first-run flow, distribution via `cargo install` shipping a single static binary that fetches the com0com installer at runtime.

I asked four risks/questions:

1. USBPcap post-install reboot UX (filter driver attaches at PnP enumeration; existing devices need replug).
2. Linux `usbmon` privilege flow — udev rule on first run vs sudo every time.
3. FT2232 multi-port resolution wrinkle (just noting).
4. Unknown VID/PID — fall back to CDC-ACM-style passthrough?

---

## 3 — User answers (round 2)

**User:** Check if com0com has signed drivers for Windows 11. If not, fall back to USBPcap.

**Assistant:** I ran web searches and reported back:

- **com0com 3.0.0.0** is signed but has confirmed **Error Code 52** failures on Windows 11 — drivers install but ports don't function.
- **com0com 2.2.2.0** (Pete Batard's signed build) **does work on Windows 11**, including with Secure Boot enabled. Multiple confirmations through 2025. Caveat: ports appear under "com0com — serial port emulators" rather than "Ports (COM & LPT)".
- The signing cert is expired but the signature was timestamped, so Windows still honors it.

**Verdict:** active-proxy + com0com is viable; pin **2.2.2.0**, not 3.0.0.0.

I argued for keeping the USBPcap fallback as `--passive` from v1 anyway: hedges against future Win11 driver-signing tightening, supports users who can't reconfigure their app, and the per-chip URB decoders (CDC-ACM, FTDI 2-byte header, CH340, PL2303) are well-documented and small.

Updated plan delta:

```
Default mode:   active proxy (com0com 2.2.2.0 / pty)
Fallback flag:  --passive    (USBPcap / usbmon, no port reconfig needed)
```

Auto-install on Windows: try com0com 2.2.2.0 first; if pair won't open (e.g. future Win11 SKU), tear down and offer auto-fallback to `--passive` with USBPcap.

---

## 4 — User answers (round 3)

**User:** Will recent versions of USBPcap cause issues with secure boot?

**Assistant:** Researched and reported:

- Latest USBPcap is **1.5.4.0** (May 2024); the repo includes a `driver_attestation_signing.bat` script — the released installer is **attestation-signed by Microsoft**.
- The April 2026 update ends trust for **legacy cross-signed kernel drivers** on Win11 24H2/25H2/26H1. Attestation-signed and WHCP-signed drivers are explicitly the *new* trust paths — USBPcap is on the safe side, not the affected one.
- 1.5.4.0 installs cleanly on Windows 11 with Secure Boot **enabled** — no test-signing mode needed. Multiple anecdotal confirmations through 2024–2025.
- ARM64 is **not** signed (open issue on the repo), so passive mode unavailable on Windows-on-ARM.

**Net effect on the plan: no change.** Both com0com 2.2.2.0 (timestamped before signature expiry) and USBPcap 1.5.4.0 (attestation-signed) sit on the trusted side of the April 2026 signing changes.

---

## 5 — User answers (round 4)

**User:** Make passive default. Embed binaries for Windows. If Windows ARM64 or Mac, display error and close.

**Assistant:** Locked in three changes:

1. **Mode inversion.** Passive (USBPcap on Windows / usbmon on Linux) is now default. Active becomes opt-in via `--active`. Per-chip URB decoders (CDC-ACM, FTDI, CH340, PL2303) ship in v1.

2. **Embedded binaries (Windows only).** Bundle USBPcap 1.5.4.0 (~330 KB) for the default passive flow and com0com 2.2.2.0 (~1.6 MB) for `--active` via `include_bytes!`. SHA-256 pinned in source. License compliance: USBPcap is BSD-2/GPLv2-driver, com0com is GPLv3 — we're redistributing installers as separate executables, not statically linking GPL code, so our crate's license is unaffected.

3. **Platform gating.**

```rust
#[cfg(target_os = "macos")]
compile_error!("serial-capture is not supported on macOS.");
#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
compile_error!("serial-capture is not supported on Windows on ARM (USBPcap has no signed ARM64 driver).");
```

Plus runtime check that bails with "Detected: <os> <arch>".

I asked four follow-up questions:

1. USBPcap post-install replug message OK?
2. Linux usbmon privilege: (a) sudo prompt on first run installing udev rule, or (b) sudo every time?
3. FT2232 multi-port wrinkle (no decision needed).
4. Unknown VID/PID falls back to CDC-ACM passthrough — OK?

---

## 6 — User answers (round 5)

**User:** 1. yes 2. (a) 4. yes

**Assistant:** Started milestone 1: Linux passive (usbmon → CDC-ACM decoder → text log).

Created tasks, then ran `cargo init --name serial-capture --bin`. Wrote:

- `Cargo.toml` with `clap`, `anyhow`, `thiserror`, `time`; pcap as Linux-only dep; windows crate as Windows-only dep.
- `src/platform_guard.rs` — compile-time `compile_error!` for macOS and Win-ARM64 + runtime fallback.
- `src/cli.rs` — clap CLI with all the agreed flags.
- `src/resolve/{mod,linux}.rs` — `/dev/ttyUSB0` → bus/devnum/VID/PID via sysfs walk.
- `src/capture/{mod,passive_linux}.rs` — usbmon via libpcap, parses `usbmon_packet` header (DLT_USB_LINUX 48 bytes / DLT_USB_LINUX_MMAPPED 64 bytes), filters by bus+devnum+bulk transfer.
- `src/decode/{mod,cdc_acm}.rs` — `Decoder` trait + CDC-ACM passthrough.
- `src/output/{mod,text}.rs` — timestamped hex+ASCII log with live tail.
- `src/main.rs` — entry, dispatcher.

`cargo build` failed: missing `libpcap-dev`.

**User:** done [installed libpcap-dev]

**Assistant:** Build clean. Smoke-tested:

```
$ ./target/debug/serial-capture --port /dev/ttyACM0 -o /tmp/cap.txt
→ Port /dev/ttyACM0: bus 1 device 13 VID:PID 1209:0002 interface 1
→ Decoder: cdc-acm
→ Logging to /tmp/cap.txt
Error: opening 'usbmon1' for capture
Caused by: libpcap error: No such device exists
```

Improved the error message to a preflight that distinguishes "module not loaded" from "permission denied" with exact `sudo modprobe usbmon` / `sudo install -m 644 …` instructions.

Milestone 1 complete: 8 tasks done, builds clean, resolver works on real hardware.

---

## 7 — Milestone 2 (Linux FTDI decoder)

**User:** Move on to milestone 2.

**Assistant:** Discovered the resolver was landing on the CDC-ACM **control** interface (only an interrupt endpoint), but the bulk endpoints live on the **data** interface. Extended the sysfs walk to scan sibling interfaces if the chosen one has no bulk endpoints.

Extended `PortInfo` with `bulk_in_ep`, `bulk_out_ep`, `bulk_in_max_packet`. Added `decode/ftdi.rs` that strips the 2-byte modem-status header per `wMaxPacketSize` chunk on bulk IN (passthrough on bulk OUT). 5 unit tests covering passthrough, chunk stripping, short trailing chunk, malformed input, high-speed 512B chunks. All pass.

`/dev/ttyACM0` now reports `Decoder: cdc-acm (bulk IN 0x83, OUT 0x04)`.

---

## 8 — Milestone 3 (Windows passive — blind)

**User:** Continue.

**Assistant:** Strategy: write the Windows code blind, verify with `cargo check --target x86_64-pc-windows-gnu`. Decided to talk to USBPcap directly via Win32 (no Npcap dependency) — simpler distribution.

Added `rustup target add x86_64-pc-windows-gnu`, then wrote:

- `src/resolve/win.rs` — registry walk under `HKLM\SYSTEM\CurrentControlSet\Enum\USB\` matching `Device Parameters\PortName`.
- `src/capture/passive_win.rs` — opens `\\.\USBPcap1` via `CreateFileW`, sends `IOCTL_USBPCAP_SETUP_BUFFER` and `IOCTL_USBPCAP_START_FILTERING` with our address bit, reads pcap-format byte stream, parses `USBPCAP_BUFFER_PACKET_HEADER` (27-byte packed), filters bulk transfers, pipes payload to existing decoder pipeline.

Cross-check passed. Linux still green.

Acknowledged: hardcoded `\\.\USBPcap1`, no endpoint discovery on Windows yet, can't end-to-end test without a Windows box.

---

## 9 — Milestone 4 (USBPcap auto-install + UAC)

**User:** Continue with milestone 4.

**Assistant:** Downloaded the installer once to compute its SHA-256: `87a7edf9bbbcf07b5f4373d9a192a6770d2ff3add7aa1e276e82e38582ccb622` (195 KB).

Wrote:

- `build.rs` — fetches `USBPcapSetup-1.5.4.0.exe` to `OUT_DIR` for Windows targets, verifies pinned SHA-256, supports `USBPCAP_INSTALLER=/path` override for offline builds.
- `Cargo.toml` — `[build-dependencies]` `ureq` + `sha2`.
- `src/elevation/{mod,win}.rs` — `is_elevated()` via `OpenProcessToken`+`GetTokenInformation(TokenElevation)`; `relaunch_self_elevated_and_wait()` via `ShellExecuteExW` verb=`runas`, blocks on `WaitForSingleObject`, returns child exit code.
- `src/install/mod.rs` — `Outcome` enum, cross-platform `ensure_capture_driver_installed()`. Linux is no-op.
- `src/install/win_usbpcap.rs` — `is_installed()` checks `HKLM\SYSTEM\CurrentControlSet\Services\USBPcap`. `install_silent()` writes the embedded bytes to `%TEMP%\serial-capture-install-<ns>\USBPcapSetup.exe`, runs with `/S`, cleans up.
- `src/main.rs` — calls install gate; on success prints replug message and exits.

Cross-check & Linux build clean.

Full Windows first-run flow:

1. User: `serial-capture --port COM4 -o cap.txt`
2. Tool detects USBPcap missing → "UAC prompt will appear..."
3. Re-launches elevated; parent waits.
4. Elevated child extracts installer, runs `/S`, verifies registry, exits 0.
5. Parent prints "Unplug and replug your USB device, then re-run."

---

## 10 — Milestone 5 (FTDI MPS on Windows)

**User:** Proceed with milestone 5.

**Assistant:** Decided against a 500-line hub-IOCTL FFI block. Implemented a **PID-based MPS heuristic** + `--ftdi-mps N` override:

| PID | Chip | MPS |
|-|-|-|
| 0x6001 | FT232AM/BM/R/L | 64 |
| 0x6010 | FT2232C/D *or* FT2232H | 512 (FT2232H assumed) |
| 0x6011 | FT4232H | 512 |
| 0x6014 | FT232H | 512 |
| 0x6015 | FT-X | 64 |

Selection precedence: **CLI override → resolver value → PID heuristic → 64**. 4 unit tests covering each tier. 9/9 tests pass.

Linux still uses the exact resolver-discovered MPS; the heuristic only kicks in on Windows where endpoint discovery isn't implemented yet.

---

## 11 — Milestone 6 (CH340 + PL2303)

**User:** Continue with milestone 6.

**Assistant:** Both chips are wrapper-free (raw bytes on bulk IN/OUT, identical to CDC-ACM). Added `decode/ch340.rs` (VID 0x1a86) and `decode/pl2303.rs` (VID 0x067b) — separate structs for accurate decoder names. Extended VID dispatch in `decode::select`. 4 new tests. 13/13 pass.

---

## 12 — Milestone 7 (pcapng output)

**User:** Go ahead with milestone 7.

**Assistant:** Wrote raw URBs to pcapng (link types `DLT_USB_LINUX_MMAPPED` / `DLT_USBPCAP`) so Wireshark's USB dissector decodes everything natively.

- `src/output/pcap.rs` — minimal pcapng writer: SHB + IDB + EPB blocks, microsecond timestamps. 3 byte-layout unit tests.
- Both passive captures tee `pkt.data` to `PcapSink` after bus+devnum match but **before** bulk/endpoint filters, so control transfers and other URBs for the target device are also captured.
- `--pcap FILE` plumbed through.

External validation:

```
$ capinfos /tmp/smoke.pcapng
File type: Wireshark/... - pcapng
Number of interfaces in file: 1
Interface #0 info: Encapsulation = USB packets with Linux header and padding
                   Capture length = 65535
                   Time precision = microseconds (6)
```

16/16 tests pass.

---

## 13 — Milestone 8 (polish + Ctrl-C)

**User:** Continue with milestone 8.

**Assistant:** Audit found `--printable-only` was inert — both branches in the ASCII-column formatter produced `.` regardless. Two real fixes:

1. `--printable-only` now skips events whose payload contains zero text bytes (printable ASCII or `\t`/`\n`/`\r`). 5 unit tests.
2. Ctrl-C handler via `ctrlc` crate: counts events, prints `"Stopped. Wrote N event(s) to <path>. Pcapng saved to <path>."` and exits 0.

Smoke-tested every flag combination simultaneously — banner reflects all settings, preflight runs before pcap file is opened (no orphan files on capture failure). 21/21 tests pass.

---

## 14 — Milestone 9a (Linux active mode)

**User:** Continue with milestone 9.

**Assistant:** Split into 9a (Linux) and 9b (Windows) since each is a meaningful chunk and Linux is testable here.

Wrote `src/capture/active_linux.rs`:

- pty pair via `posix_openpt` / `grantpt` / `unlockpt` / `ptsname_r` (nix crate).
- Real port opened with `O_NOCTTY`.
- Both endpoints set to raw termios.
- Two forwarder threads: `pty→real` (Direction::Out) and `real→pty` (Direction::In) tee Events to `Arc<Mutex<TextSink>>`.
- `--baud` configures the real port via `cfsetspeed`.
- `--pcap` rejected with `--active` (no URB stream to record).

Smoke-tested with a Python harness that opens a pty as the simulated "real device", spawns serial-capture against the slave path, then exercises both directions through the exposed pty:

```
got at device: b'hello-from-app\n'        # app→device byte-exact
got at app:    b'hello-from-device\n'     # device→app byte-exact
Stopped. Wrote 2 event(s) to /tmp/active_cap.txt.

--- log ---
17:35:20.878  →  68 65 6c 6c 6f 2d 66 72 6f 6d 2d 61 70 70 0a     |hello-from-app.|
17:35:21.078  ←  68 65 6c 6c 6f 2d 66 72 6f 6d 2d 64 65 76 69 63  |hello-from-devic|
17:35:21.078  ←  65 0a                                            |e.|
```

Bugs caught and fixed during the smoke test:

- **Line-discipline mangling** (`\n` → `\r\n`): fixed by setting raw termios on the pty master, not just the real port.
- **Echo loop** (writes to master got re-read on master via slave's tty): same fix; raw mode disables ECHO.

---

## 15 — Milestone 9b (Windows active mode)

**User:** Go with milestone 9.

**Assistant:** Investigated the com0com archive layout (driver files + `setupc.exe`, no clean silent installer like USBPcap). Decided to scope down 9b: detect via registry, print clear manual install instructions if missing, implement the bridge itself. Auto-install (extract → `pnputil /add-driver` → `setupc install`) deferred to a future milestone where it can be validated live.

Wrote:

- `src/install/win_com0com.rs` — registry detection at `HKLM\SYSTEM\CurrentControlSet\Services\com0com`; `print_install_instructions()` with Pete Batard 2.2.2.0 URL + `pnputil`/`setupc` commands.
- `src/capture/active_win.rs` — opens `CNCB0` (default pair end) + the real COM port via `serialport-rs`; `try_clone()` for split read/write; two threads bridge with the same `Direction::Out` / `Direction::In` semantics as Linux active.
- `Cargo.toml` — `serialport = "4"` Windows-only.
- `src/main.rs` — Windows `--active` dispatch.

Cross-check passed. 21/21 Linux tests still pass.

---

## 16 — README + initial commit

**User:** Add README.md and commit.

**Assistant:** Wrote `README.md` covering quick-start, install, platform-support matrix, both modes (passive default vs `--active`), output format, first-run requirements per platform/mode (USBPcap auto-install + replug, com0com manual steps with the Pete Batard URL, Linux udev rule), full CLI reference, ASCII architecture diagram, "limitations / future work" list, license, and acknowledgements.

Cleaned up a stray `cap.txt` from a smoke test, then:

```
$ git add .gitignore Cargo.lock Cargo.toml README.md build.rs src/...
$ git commit -m "Initial commit: cross-platform USB virtual COM port capture"
[main 7d5b409] Initial commit: cross-platform USB virtual COM port capture
 29 files changed, ...
```

---

## 17 — Live USBPcap bug #1: ERROR_ACCESS_DENIED

**User:**

```
Error: USBPcap SETUP_BUFFER ioctl failed: Access is denied. (0x80070005)
```

**Assistant:** Real bug. `IOCTL_USBPCAP_SETUP_BUFFER` is defined with `FILE_READ_ACCESS | FILE_WRITE_ACCESS`, but I open `\\.\USBPcap1` with only `GENERIC_READ`. The Windows I/O manager validates the IOCTL's access bits against the handle's access rights — read-only handle + read-write IOCTL = access denied.

Fixed: changed to `GENERIC_READ.0 | GENERIC_WRITE.0`. Note this isn't about admin/elevation — opening for read+write doesn't need an Administrator token; only USBPcap's *installer* does.

Committed `5a87988`.

---

## 18 — Live USBPcap bug #2: ERROR_INVALID_FUNCTION

**User:**

```
Error: USBPcap SETUP_BUFFER ioctl failed: Incorrect function. (0x80070001)
```

**Assistant:** Driver didn't recognize the IOCTL code I sent. Pulled the 1.5.4.0 source tarball and read `USBPcapDriver/include/USBPcap.h`:

```c
#define IOCTL_USBPCAP_SETUP_BUFFER \
    CTL_CODE(FILE_DEVICE_UNKNOWN, 0x800, METHOD_BUFFERED, FILE_READ_ACCESS)

#define IOCTL_USBPCAP_START_FILTERING \
    CTL_CODE(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS)
```

USBPcap uses **`FILE_DEVICE_UNKNOWN` (0x22)**, not `FILE_DEVICE_USBPCAP` (0x8001) as I'd assumed. Recomputed:

```
SETUP_BUFFER     = (0x22 << 16) | (1 << 14) | (0x800 << 2) | 0 = 0x0022_6000
START_FILTERING  = (0x22 << 16) | (3 << 14) | (0x801 << 2) | 0 = 0x0022_E004
```

(Previous incorrect values: `0x8001E000` / `0x8001E004`.)

Committed `4fbb3b3`.

---

## 19 — Live USBPcap bug #3: ERROR_INVALID_PARAMETER

**User:**

```
Error: USBPcap START_FILTERING ioctl failed: The parameter is incorrect. (0x80070057)
```

**Assistant:** Read the `USBPCAP_ADDRESS_FILTER` definition:

```c
#pragma pack(push)
#pragma pack(1)
typedef struct _USBPCAP_ADDRESS_FILTER
{
    UINT32 addresses[4];
    BOOLEAN filterAll;
} USBPCAP_ADDRESS_FILTER, *PUSBPCAP_ADDRESS_FILTER;
#pragma pack(pop)
```

The struct is `#pragma pack(1)` so its on-the-wire size is exactly **17 bytes** (16 + 1, no tail padding). My Rust struct was `#[repr(C)]` with default alignment — `size_of` returned **20** (3 bytes of trailing pad to align the trailing `u8` to a 4-byte boundary). The driver validates `InputBufferLength` against the packed size and rejected 20.

Fix:

```rust
#[repr(C, packed)]
struct UsbpcapAddressFilter {
    addresses: [u32; 4],
    filter_all: u8,
}

const _: () = assert!(std::mem::size_of::<UsbpcapAddressFilter>() == 17);
```

Field initialization rewritten to build `addresses` in a local first, since taking references to packed fields is UB in Rust.

Committed `b0468ce`.

---

## 20 — Closing notes

Three USBPcap-side bugs in a row, each instructive:

1. `GENERIC_READ` only → `ERROR_ACCESS_DENIED` on a `FILE_WRITE_ACCESS` IOCTL
2. Wrong device-type magic in `CTL_CODE` → `ERROR_INVALID_FUNCTION`
3. `repr(C)` not `repr(C, packed)` → `ERROR_INVALID_PARAMETER`

Each was caught by reading the driver's actual public header instead of guessing. Lesson: when interfacing with a kernel driver, fetch the source and treat its public header as ground truth — Win32 errors at the IOCTL layer are precise enough to localize the bug if you know what to look for.

**Commit log so far:**

```
b0468ce Fix USBPCAP_ADDRESS_FILTER size: repr(C, packed) for 17-byte layout
4fbb3b3 Fix USBPcap IOCTL codes (FILE_DEVICE_UNKNOWN, not FILE_DEVICE_USBPCAP)
5a87988 Fix USBPcap access-denied: open handle with GENERIC_READ|GENERIC_WRITE
7d5b409 Initial commit: cross-platform USB virtual COM port capture
```

**Outstanding future work** (deliberately not in v1):

- Hub-IOCTL endpoint discovery on Windows (replaces the FTDI MPS heuristic with the actual `wMaxPacketSize` from the parent hub via `IOCTL_USB_GET_NODE_CONNECTION_INFORMATION_EX`).
- com0com auto-install on Windows (extract Pete's archive, `pnputil /add-driver`, `setupc install`).
- Serial state mirroring in active mode (DTR/RTS/parity/break — needed for Arduino auto-reset workflows).
- udev rule auto-installer on Linux (currently we print exact commands).
- Multi-controller `\\.\USBPcapN` selection (currently hardcoded to USBPcap1).
- `--pcap` in active mode (would need a synthetic DLT_USER0 framing with direction prefix).

---

## 21 — Test suite design (post-IOCTL fixes)

**User:** Okay, I have an Arduino Uno. Write a test suite that I can run on Windows + firmware for Arduino Uno to go with the test suite.

**Assistant:** Three USBPcap IOCTL fixes shipped without an end-to-end confirmation that capture actually works post-fix. Right next step. Scoped to **passive mode only**; active mode needs com0com configured and is a separate chunk.

Three files:

- `tests/uno_firmware/uno_firmware.ino` — deterministic patterns at 115200 baud:
  - `PING` → `PONG\r\n`
  - `BURST <N>` → `BURST:` + N bytes (`i & 0xFF`) + `\r\n`
  - `BIN <N>` → N non-printable bytes (`0x80 | i & 0x7F`), `Serial.flush()` + 50 ms (so the 16U2 ships the binary URB before the trailing marker), then `END\r\n`
  - `LARGE` → 512 × `'A'` + `\r\n`
  - fallback: `ECHO:<line>\r\n`

- `tests/run_tests.py` — Python + pyserial harness, eight tests:

  | name             | exercises                                                       |
  |------------------|-----------------------------------------------------------------|
  | passive_smoke    | start, clean Ctrl-Break exit, `Stopped. Wrote N` summary        |
  | ping_pong        | both directions captured                                        |
  | large_payload    | multi-URB reassembly (the key thing post-IOCTL fixes)           |
  | binary_payload   | non-printable bytes recovered through the hex column            |
  | printable_only   | filter skips events with zero text bytes                        |
  | pcap_output      | `--pcap` produces a structurally valid pcapng (SHB magic check) |
  | format_hex       | hex only, no `\|...\|` ASCII column                             |
  | format_ascii     | ASCII only, no hex pairs                                        |

- `tests/README.md` — setup + run.

Design choices:

- **Ctrl-C delivery.** Spawn with `CREATE_NEW_PROCESS_GROUP`; send `CTRL_BREAK_EVENT` so the `ctrlc` handler fires and the summary line is printed (we assert on it).
- **Race-free start.** Each test blocks on the `→ Logging to …` banner before opening the Uno.
- **Binary URB isolation.** `Serial.flush()` + 50 ms in firmware so the BIN response and the trailing `END\r\n` go in separate USB packets — that's what makes `--printable-only` testable; otherwise the printable trailing line keeps the whole event around.
- **Reset tolerance.** pyserial port-open asserts DTR which auto-resets the Uno. Each test sleeps 2 s after open and drops the boot banner before sending commands.

Pushed `b50c39d`.

---

## 22 — First run: 0/8, then 7/8 with two harness bugs surfacing

**User:** [pasted output] 0/8 passed; every test reports `capture did not start within timeout. output:` with empty captured output.

**Assistant:** Empty output means either the binary is exiting silently or Python isn't decoding what it prints. Asked the user to run the binary directly to disambiguate; in parallel, hardened the harness:

- `_drain` reader thread wraps in try/except so it can't die quietly.
- subprocess decodes as `encoding='utf-8'`, `errors='replace'` (Python on Windows defaults to cp1252 for `text=True` mode).
- the "did not start" error includes the process exit code.

**User:** [re-ran] 7/8 passed. Each test had a stack trace from `_drain`:

```
File "C:\...\encodings\cp1252.py", line 23, in decode
    return codecs.charmap_decode(input, self.errors, decoding_table)[0]
UnicodeDecodeError: 'charmap' codec can't decode byte 0x90 in position 27:
character maps to <undefined>
```

But — somehow — the tests still passed.

**Assistant:** Two findings.

**(a) The result is good news.** Seven of eight tests passed against real bytes captured from the Uno through USBPcap. ping_pong, large_payload (512-byte multi-URB reassembly), binary_payload, printable_only, pcap_output, format_hex, format_ascii — all read real captured bytes from log files. **The IOCTL fixes work end-to-end.** Most tests didn't care about stdout content (they assert on the log file), so a dead reader thread didn't break them.

**(b) The traceback line numbers were the *unpatched* code.** `line 59, in _drain ... for line in iter(stream.readline, ""):` is the original `_drain`, not the try/except version. The Windows VM was reading a stale copy through the VBox shared folder. Pushed `f079ac3` to git so a `git pull` past the cache picked up the encoding fix.

The encoding bug itself decoded:

- The live tail prints `←` for device→host events. UTF-8 of `←` is `0xE2 0x86 0x90`.
- `0x90` is **undefined** in cp1252 (not just unused — explicitly undefined). With Python's default `errors='strict'` for `text=True`, this raises `UnicodeDecodeError`.
- Position 27 = 25 chars of `2026-MM-DD HH:MM:SS.mmm  ` + `0xE2 0x86` — the next byte is `0x90`. Match.

---

## 23 — The 8th test: await loop was eating the preamble

**User:** [pasted output, post-pull] 7/8 passed; `passive_smoke` still failing.

**Assistant:** Captured stdout was now clean (no mangled arrows — encoding fix landed) but only contained:

```
→ Press Ctrl-C to stop.

Stopped. Wrote 0 event(s) to ...
```

The earlier `→ Port`, `→ Decoder`, `→ Logging to` lines were missing. They run sequentially in main.rs and "Press Ctrl-C" prints after them, so they had to have been written. Where did they go?

The harness:

```python
def await_capture_active(proc, q, timeout=15.0):
    while ...:
        line = q.get(timeout=0.2)        # consumes from queue
        if "Logging to" in line:
            return True
```

`q.get()` removes lines from the queue. Lines preceding `Logging to` (Port, Decoder) were pulled, scanned, **discarded**. Only what arrived after — `Press Ctrl-C` and the summary — survived for `stop_capture` to drain. The smoke test's `'Decoder:' in sess.output` check failed because the line had been silently consumed.

Fix: the await loop appends each consumed line to a session-owned preamble buffer; `Session.stop()` prepends it to the post-stop drain. Full stdout preserved.

Pushed `20e0138`.

**User:** Works well!

8/8 passed.

---

## 24 — End-to-end validation complete

The three USBPcap IOCTL fixes (`5a87988`, `4fbb3b3`, `b0468ce`) are now confirmed end-to-end on real Arduino Uno hardware on Windows: bytes flow in both directions, 512-byte payloads spanning multiple bulk URBs reassemble correctly, non-printable bytes survive the hex column, `--printable-only` filters as intended, and `--pcap` produces structurally valid pcapng files.

Two harness bugs surfaced and were fixed in the same session:

1. **cp1252 vs. UTF-8 byte `0x90`** — the live tail's `←` contained an undefined byte for cp1252; the reader thread died silently on first device→host event. Most tests still passed because they assert on the log file, not stdout. Fixed in `f079ac3`.
2. **await loop ate the preamble** — `q.get()` consumed startup banner lines before assertions could see them. Fixed in `20e0138`.

The "Outstanding future work" list from §20 is unchanged — none of those items were touched this round.

**Final commit log:**

```
20e0138 Preserve preamble lines consumed by await_capture_active
f079ac3 Fix harness reader thread dying on cp1252 + Windows live tail
b50c39d Add Windows end-to-end test suite with Arduino Uno firmware
b0468ce Fix USBPCAP_ADDRESS_FILTER size: repr(C, packed) for 17-byte layout
4fbb3b3 Fix USBPcap IOCTL codes (FILE_DEVICE_UNKNOWN, not FILE_DEVICE_USBPCAP)
5a87988 Fix USBPcap access-denied: open handle with GENERIC_READ|GENERIC_WRITE
7d5b409 Initial commit: cross-platform USB virtual COM port capture
```

