# IPICO Reader Protocol Reference

> Last reviewed: 2026-03-07

This document consolidates what this repository currently knows about the IPICO
reader protocol. It covers:

- The ASCII-hex control and tag protocol observed on TCP port `10000`
- The legacy serial specification in
  [`IPICO-Reader-Serial-Protocol-100-20071120.pdf`](./IPICO-Reader-Serial-Protocol-100-20071120.pdf)
- The packet captures under [`captures/`](./captures/)
- The exported download artifact [`captures/testreads`](./captures/testreads)
- The local decoder in [`scripts/parse_pcap.py`](../../scripts/parse_pcap.py)
- The local Rust implementation in
  [`crates/ipico-core/src/control.rs`](../../crates/ipico-core/src/control.rs)

The goal is not to flatten every source into one confidence level. This file
tries to keep confirmed wire behavior separate from spec-only material and from
open questions.

## Quick Navigation

- [Evidence Levels](#evidence-levels)
- [Sources](#sources)
- [Reader Under Test](#reader-under-test)
- [Capture Inventory](#capture-inventory)
- [Protocol Families](#protocol-families)
- [Transport and Framing](#transport-and-framing)
- [Startup Banner](#startup-banner)
- [Tag Report Frames (`aa`)](#tag-report-frames-aa)
- [Control Command Reference](#control-command-reference)
- [Observed Session Behavior](#observed-session-behavior)
- [Appendix: PDF-Documented Host Commands Not Seen In These Captures](#appendix-pdf-documented-host-commands-not-seen-in-these-captures)
- [Known Mismatches Between the 2007 PDF and the 2013 Reader Captures](#known-mismatches-between-the-2007-pdf-and-the-2013-reader-captures)
- [Open Questions](#open-questions)
- [Power-On and Power-Off Behavior](#power-on-and-power-off-behavior)
- [Practical Guidance for Future Work](#practical-guidance-for-future-work)

## Evidence Levels

- `Capture`: directly observed in the `.pcapng` files in this repo
- `Spec`: described in the 2007 serial protocol PDF
- `Capture + Spec`: seen on the wire and described by the PDF
- `Inference`: strong interpretation from captures, code, or both
- `Unknown`: seen on the wire but not yet explained

## Sources

- [`captures/connect.pcapng`](./captures/connect.pcapng)
- [`captures/con-dis.pcapng`](./captures/con-dis.pcapng)
- [`captures/settime.pcapng`](./captures/settime.pcapng)
- [`captures/delete-records.pcapng`](./captures/delete-records.pcapng)
- [`captures/guntime.pcapng`](./captures/guntime.pcapng)
- [`captures/read4tags.pcapng`](./captures/read4tags.pcapng)
- [`captures/con-dis-w-4k-reads.pcapng`](./captures/con-dis-w-4k-reads.pcapng)
- [`captures/download-events.pcapng`](./captures/download-events.pcapng)
- [`captures/downloadreads.pcapng`](./captures/downloadreads.pcapng)
- [`captures/direct-fslsreads-con-dis.pcapng`](./captures/direct-fslsreads-con-dis.pcapng)
- [`captures/direct-raw-reads-con-dis.pcapng`](./captures/direct-raw-reads-con-dis.pcapng)
- [`captures/record-on-off.pcapng`](./captures/record-on-off.pcapng)
- [`captures/testreads`](./captures/testreads)
- [`captures/turnon-con-dis.pcapng`](./captures/turnon-con-dis.pcapng)
- [`captures/setclock.pcapng`](./captures/setclock.pcapng)
- [`IPICO-Reader-Serial-Protocol-100-20071120.pdf`](./IPICO-Reader-Serial-Protocol-100-20071120.pdf)
- [`scripts/parse_pcap.py`](../../scripts/parse_pcap.py)

## Reader Under Test

The captures in this repo are from the same reader family and the same observed
reader instance:

- Banner: `ARM9 Controller for DF Dual DSP TTO Actel FPGA (STK Lite) (38.4kB) v1.4 Jun  5 2013 14:16:40 (RWXLF)`
- Reader IP: `192.168.0.155`
- Control TCP port: `10000`
- Reader ID on the wire: `0x00`

The 2007 PDF predates this 2013 ARM9 firmware. Several commands and reply
variants in the captures are therefore later-firmware extensions.

## Capture Inventory

| Capture | What it shows |
| --- | --- |
| `docs/ipico-protocol/captures/connect.pcapng` | Full TCP bootstrap, status polling, filter writes, clock set, read-mode changes |
| `docs/ipico-protocol/captures/con-dis.pcapng` | Full bootstrap, steady-state polling, disconnect |
| `docs/ipico-protocol/captures/settime.pcapng` | Full bootstrap, single clock set + verify |
| `docs/ipico-protocol/captures/delete-records.pcapng` | Full bootstrap, record-clear sequence via `0x4b` |
| `docs/ipico-protocol/captures/guntime.pcapng` | Mid-stream trigger-button / gun-time event during a live session |
| `docs/ipico-protocol/captures/read4tags.pcapng` | Mid-stream `aa` tag reports plus concurrent polling traffic |
| `docs/ipico-protocol/captures/con-dis-w-4k-reads.pcapng` | Dashboard connect/disconnect while stored reads were already present; confirms non-empty idle `0x4b` status |
| `docs/ipico-protocol/captures/download-events.pcapng` | Download-events workflow via `0x4b` sub-commands; memory was empty so no records streamed |
| `docs/ipico-protocol/captures/downloadreads.pcapng` | Stored-read download via `0x4b`; shows 4,102 archived reads streamed as `aa` frames plus periodic progress/status replies |
| `docs/ipico-protocol/captures/direct-fslsreads-con-dis.pcapng` | Direct read session in FSLS mode; only one `aa` frame was captured and it had no `FS` / `LS` suffix |
| `docs/ipico-protocol/captures/direct-raw-reads-con-dis.pcapng` | Direct read session in raw mode; `aa` traffic only, with the same 36-character layout seen elsewhere |
| `docs/ipico-protocol/captures/event-read.pcapng` | Forwarder-backed live read capture while the reader reported event mode (`0305`); shows one initial `aa` frame followed by a delayed resend of the same embedded timestamp about 5.5s later |
| `docs/ipico-protocol/captures/fsls-event-tto.pcapng` | Combined FSLS/event capture with TTO off/on transitions; confirms the `0x11` TTO bit toggles 36-character vs 42-character `aa` frames and exposes non-zero FSLS status bytes only in `0505` mode |
| `docs/ipico-protocol/captures/record-on-off.pcapng` | Record-off then record-on toggle via `0x4b` + CONFIG3 mode changes |
| `docs/ipico-protocol/captures/turnon-con-dis.pcapng` | Full power-on, connect, poll, disconnect; confirms bootstrap sequence after fresh boot |
| `docs/ipico-protocol/captures/setclock.pcapng` | Five consecutive SET_DATE_TIME attempts via the forwarder; confirms SET takes effect at next cs rollover, not immediately |
| `docs/ipico-protocol/captures/tto-enable.pcapng` | Two TTO-enable attempts via `0x11` query; reader returns 10-byte format response (`LL=0a`) instead of the 8-byte form seen in earlier captures |

## Protocol Families

There are three distinct message families relevant to this repo.

### 1. Control / management frames (`ab`, optionally `ac`)

`Capture + Spec`

These are the framed request/reply messages used for clock management, status,
configuration, filtering, and other control functions.

### 2. Tag report frames (`aa`)

`Capture + Spec`

These are unsolicited tag-read reports on the same TCP connection. The exact
report layout is configurable by command `0x11`; the captures in this repo show
one concrete ASCII layout.

### 3. Plain ASCII banner lines

`Capture + Spec`

The reader can emit plain text startup/banner lines. Over TCP we observed these
when command `0x37` is sent.

## Transport and Framing

### Transport

- `Capture`: all captures use a single TCP session to port `10000`
- `Capture`: control frames, tag reports, and banner text all share that same
  TCP stream
- `Capture`: unsolicited traffic can arrive interleaved with replies
- `Spec`: the same ASCII-hex framing also exists on the reader's serial
  interface

### Control frame format

`Capture + Spec`

```text
ab  RR  LL  II  [DD...]  CC  \r\n
```

| Field | Chars | Meaning |
| --- | --- | --- |
| `ab` | 2 | Normal control header |
| `ac` | 2 | Terminal-mode header; skips request-side LRC checking |
| `RR` | 2 | Reader ID in ASCII hex; `00` is broadcast |
| `LL` | 2 | Data length in bytes, in ASCII hex |
| `II` | 2 | Instruction / ACK / error byte |
| `DD` | `LL * 2` | Data payload bytes, rendered as lowercase ASCII hex |
| `CC` | 2 | LRC checksum |
| `\r\n` | 2 bytes | Frame terminator |

Notes:

- `Spec`: request payloads are documented as up to 10 bytes in the 2007 spec
- `Spec`: reply payloads are documented as up to 15 bytes
- `Capture`: later firmware appends extra reply bytes where needed
- `Capture + Spec`: `LL = ff` is used as a query mode for some commands
- `Spec`: early firmware up to version 5.1 may reply with `aa` instead of `ab`
  for control frames; none of our captures show that behavior

### Query mode (`LL = ff`)

`Capture + Spec`

`ff` is not a literal 255-byte payload. It is a special "return current value"
query marker on commands that support get/set behavior.

Confirmed in this repo:

- `Capture`: `0x09` CONFIG3 query
- `Capture`: `0x4b` extended-status query
- `Spec`: `0x30` select-pattern query is supported by the protocol, though only
  writes were seen in the captures
- `Spec`: `0x31` select-mask query is supported by the protocol, though only
  writes were seen in the captures
- `Spec`: `0x11`, `0x30`, `0x31`, `0x32`, and `0x33` support query mode on
  later firmware

### LRC checksum

`Capture + Spec`

The checksum is the low byte of the sum of the ASCII byte values of every
character between the header and the checksum field itself.

Example:

```text
LRC("000002") = 0x30 + 0x30 + 0x30 + 0x30 + 0x30 + 0x32 = 0x122 -> 0x22
```

The same rule is used for `aa` tag frames: sum the ASCII bytes after the `aa`
header and before the checksum field.

### ACK and error replies

`Capture + Spec`

An ACK is a normal control reply with:

- The same instruction byte as the request
- Length `00`
- No data

Example:

```text
ab00000121\r\n
```

Error codes:

| Code | Meaning |
| --- | --- |
| `0xf0` | Bad length (`> 10`) |
| `0xf1` | Bad LRC |
| `0xf2` | Bad / unknown instruction |
| `0xf4` | Unsupported command |
| `0xf5` | Unsupported sub-command |

## Startup Banner

### Observed banner behavior

`Capture + Spec`

Command `0x37` causes the reader to emit a plain ASCII banner line followed by
an ACK frame.

Observed banner:

```text
ARM9 Controller for DF Dual DSP TTO Actel FPGA (STK Lite) (38.4kB) v1.4 Jun  5 2013 14:16:40 (RWXLF)
```

Observed exchange:

```text
C->R  ab0000372a\r\n
R->C  \r\n
R->C  ARM9 Controller for DF Dual DSP TTO Actel FPGA (STK Lite) (38.4kB) v1.4 Jun  5 2013 14:16:40 (RWXLF)\r\n
R->C  \r\n
R->C  ab0000372a\r\n
```

### Banner modifier codes

`Spec`

The 2007 PDF defines many banner suffix modifiers. The ones relevant to the
observed `RWXLF` suffix are:

- `RW`: read/write firmware
- `X`: pulse Aux1 when a tag is seen or when an ID is sent
- `L`: Aux1 active low
- `F`: FPGA decoder is being used

The older reverse-engineering note in this repo treated these letters as
separate unknowns. The PDF gives the stronger interpretation above.

## Tag Report Frames (`aa`)

### Current observed layout

`Capture + Spec`

The `read4tags.pcapng` capture shows the reader emitting ASCII tag reports in
this format:

```text
aa  RR  TTTTTTTTTTTT  IIQQ  YYMMDD  HHMMSS  CC  KK  \r\n
```

| Field | Chars | Meaning |
| --- | --- | --- |
| `aa` | 2 | Tag-report header |
| `RR` | 2 | Reader ID |
| `TTTTTTTTTTTT` | 12 | Tag ID with the tag CRC bytes omitted |
| `IIQQ` | 4 | I-channel and Q-channel counters |
| `YYMMDD` | 6 | Date |
| `HHMMSS` | 6 | Time |
| `CC` | 2 | 10 ms counter, rendered as hex |
| `KK` | 2 | LRC over chars 2..33 |

Example:

```text
aa0005800012860800012603062027472353
```

Which decodes to:

- Reader ID: `00`
- Tag ID: `058000128608`
- I/Q counters: `0001`
- Timestamp: `2026-03-06T20:27:47.350`
- LRC: `0x53`

### I/Q counters

`Spec`

The 2007 PDF's standard ASCII tag format defines the 4-character field after
the tag ID as the I-channel and Q-channel counters. In the captures here it is
usually `0001` and once `0002`. This is much stronger than the earlier
"unknown 4-byte field" interpretation.

### Timestamp encoding

`Capture + Spec`

- The date and time digits are decimal ASCII
- The `CC` field is a hex-encoded 10 ms counter
- `0x23` means 35 * 10 ms = 350 ms
- `0x3a` means 58 * 10 ms = 580 ms

### Configurability via `0x11`

`Spec + Capture`

Command `0x11` configures the tag report format. The captures in this repo
match the standard ASCII format described by the PDF:

- Header bytes `61 61` -> literal `aa`
- Trailer bytes `0d 0a` -> `CRLF`
- Reader ID included
- I/Q counters included
- Date and time included
- 10 ms counter included
- LRC included
- Tag CRC bytes omitted from the tag ID field

Earlier captures showed the common 36-character form without TTO bytes. The new
`fsls-event-tto.pcapng` capture shows both variants on the same reader/firmware:

- `Capture`: `0x11` query returns `7f fc 61 61 aa 00 0d 0a 00 00` and the
  resulting `aa` frames are 36 characters long (no TTO fields)
- `Capture`: `0x11` query returns `ff fc 61 61 aa 00 0d 0a 00 00` and the
  resulting `aa` frames are 42 characters long, with 3 extra bytes inserted
  before the LRC

The 2007 spec (section 7.15) defines parameters 0-8 (up to 9 bytes), where
parameter 8 (separator) was added in firmware version 6.4.

### `0x11` query response length varies by firmware

`Capture`

The `0x11` query (`LL=ff`) returns the current format in the same parameter
layout as the set command. The response length varies by firmware:

- `Capture`: earlier captures show 8 data bytes (`LL=08`):
  `7f fc 61 61 aa 00 0d 0a` (no separator)
- `Capture`: `tto-enable.pcapng` shows 10 data bytes (`LL=0a`):
  `7f fc 61 61 aa 00 0d 0a 00 00` — the first 9 match parameters 0-8 from the
  spec (separator = `0x00`), and the 10th byte (`0x00`) is an undocumented
  extension from this 2013 ARM9 firmware (v1.4)

Consumers should accept 8 or more data bytes and ignore any beyond the 9th

### TTO-enabled `aa` frame layout is now observed

`Capture + Spec`

`docs/ipico-protocol/captures/fsls-event-tto.pcapng` shows the TTO-enabled
ASCII layout directly on the wire. When `0x11` parameter 0 is `0xff`, the
reader sends:

```text
aa RR TTTTTTTTTTTT IIQQ yymmdd hhmmss cc iippst LL
```

Where:

- `RR` = reader ID
- `TTTTTTTTTTTT` = tag ID without CRC
- `IIQQ` = I/Q counters
- `cc` = 10 ms count
- `ii` = TTO index byte
- `pp` = TTO page byte
- `st` = TTO status / tamper byte
- `LL` = LRC

Observed examples:

- `aa00058000123b3200012603081222022f060080cd`
- `aa000580001286080001260308122253100c00009d`

### First/Last-seen reporting

`Spec + Capture`

The PDF says First/Last-seen state is carried in the TTO tamper byte when TTO
fields are enabled:

- Bit 7 -> first seen
- Bit 6 -> last seen
- Bit 0 -> tamper

The new capture adds an observed split between FSLS and event mode:

- `Capture`: with `CONFIG3 = 0505` and TTO enabled, the added bytes look like
  `060080`, `060060`, `070080`, `080060`
- `Capture`: with `CONFIG3 = 0305` and TTO enabled, the added bytes look like
  `0c0000`, `0d0000`, `0e0000`

Observed implications:

- Byte 1 behaves like the spec's TTO index byte and increments across reads
- Byte 2 remains `00`, which matches the spec's "page 0 = tag ID" rule
- Byte 3 is non-zero only in FSLS mode, strongly indicating this is the
  First/Last-seen status byte described by the spec
- The observed delayed FSLS value is `0x60`, not the spec's clean `0x40`, so
  there is still one undocumented status bit involved on this firmware

### Important local convention note

`Inference`

This repository's local parser and emulator currently model FSLS mode as a
literal `FS` or `LS` suffix on the end of an `aa` frame. That convention exists
in local code and tests, but it is not backed by the 2007 PDF and it is not
present in any of the captures in this repo. Treat it as a repo-local
assumption, not a confirmed reader protocol rule.

The new direct-read captures strengthen that warning:

- `Capture`: `direct-raw-reads-con-dis.pcapng` still shows only ordinary
  36-character `aa` frames
- `Capture`: `direct-fslsreads-con-dis.pcapng` shows one FSLS-session `aa`
  frame, `aa00058000120e38000e26030713560136b0`, and it also has no literal
  `FS` / `LS` suffix

### Observed timeout-driven resend behavior without TTO bytes

`Capture + Inference`

Two newer captures make the practical behavior clearer:

- `Capture`: `fsls multiple reads.pcapng` queries `0x09` and the reader reports
  `0505`, then sends one `aa` frame immediately followed by two more
  byte-for-byte identical `aa` frames about 5.7 seconds later
- `Capture`: `event-read.pcapng` queries `0x09` and the reader reports `0305`,
  then sends one `aa` frame immediately followed by one more `aa` frame about
  5.5 seconds later
- `Capture`: in `event-read.pcapng`, the delayed resend carries the same
  embedded read timestamp as the first message (`20:53:42.500`), with only the
  `IIQQ` field changing from `0001` to `000b`

Observed implication:

- Without TTO bytes enabled, the on-wire `aa` format in these captures does not
  provide an explicit FS/LS marker
- On this reader/firmware, both `0305` (event) and `0505` (first/last seen)
  exhibit timeout-driven follow-up traffic after the initial read
- The `0505` capture shows more than one delayed follow-up, so consumers should
  not assume FSLS will produce exactly two messages

This does not prove that event mode is semantically identical to FSLS; it does
show that the reader's observed TCP behavior is timeout-driven in both modes
when TTO bytes are absent, which is weaker and more ambiguous than the PDF's
clean distinction between event and first/last-seen.

### TTO clarifies semantics but does not change timeout behavior

`Capture + Inference`

`docs/ipico-protocol/captures/fsls-event-tto.pcapng` makes the practical role
of the `0x11` TTO bit much clearer:

- `Capture`: switching `0x11` parameter 0 from `0x7f` to `0xff` changes the
  on-wire `aa` format from 36 to 42 characters
- `Capture`: the same capture still shows delayed follow-up traffic in both
  `0505` and `0305`, so the TTO bit does not itself change the timeout/resend
  behavior
- `Inference`: the `0x11` TTO flag is a report-format toggle that exposes extra
  metadata; it is not a separate read mode
- `Inference`: this is the first in-repo capture that shows an on-wire FS/LS
  indicator, and it appears in the TTO status byte rather than as a literal
  `FS` / `LS` suffix

### Stored-read downloads use the same `aa` format

`Capture`

`downloadreads.pcapng` shows the reader streaming stored records as ordinary
36-character `aa` frames once `0x4b [01, 01]` starts the download. No separate
download-only binary record envelope was observed on the wire.

Observed facts:

- The download streamed 4,102 `aa` frames
- The frames use the same reader ID / tag ID / `IIQQ` / timestamp / checksum
  layout as live raw reads
- The exported `docs/ipico-protocol/captures/testreads` file uses that same
  ASCII `aa` line format
- The downloaded records preserve original read timestamps rather than being
  rewritten to download-time timestamps

## Control Command Reference

### 0x01 / 0x02 - RTC date and time

#### 0x01 - SET_DATE_TIME

`Capture + Spec`

Request:

```text
ab  RR  07  01  YY MM DD DW HH MM SS  CC  \r\n
```

Fields:

| Byte | Meaning | Encoding |
| --- | --- | --- |
| 0 | Year | BCD, century omitted |
| 1 | Month | BCD |
| 2 | Day of month | BCD |
| 3 | Day of week | `0..6`, with `Monday = 1` and `Sunday = 0` |
| 4 | Hour | BCD, 24-hour |
| 5 | Minute | BCD |
| 6 | Second | BCD |

Observed example:

```text
ab00070126030605194921f8\r\n
```

This sets the clock to `2026-03-06 19:49:21`, with day-of-week `05`
(Friday).

Reply:

```text
ab00000121\r\n
```

Notes:

- `Spec`: the 2007 document describes this as setting the RTC and forcing the
  RTC interrupt to 1-second intervals
- `Capture`: after a successful clock set, this reader emitted one unsolicited
  `0x4c` frame, then the management software read back `0x02` to verify
- `Capture` (`setclock.pcapng`, `another-clockset.pcapng`): SET_DATE_TIME does
  **not** take effect immediately. The centisecond counter is **reset to ~52
  (0x34)** upon receipt of the command, regardless of its prior value. The new
  second value is applied at the next cs rollover (when cs wraps from 99 → 0),
  which occurs ~480ms after the reset. The reader emits an unsolicited `0x4c`
  frame confirming a 500ms sync delay (offset field = `0x01F4`). Verify reads
  performed before the rollover show the **pre-SET** second value with the
  reset centisecond counter. The effective reader clock after the SET is
  `S.000` at the rollover moment, ~500ms after the command is received

#### 0x02 - GET_DATE_TIME

`Capture + Spec`

Request:

```text
ab00000222\r\n
```

Reply payload layout:

| Byte | Meaning | Encoding |
| --- | --- | --- |
| 0 | Year | BCD |
| 1 | Month | BCD |
| 2 | Day of month | BCD |
| 3 | Day of week | `0..6`, with `Monday = 1` and `Sunday = 0` |
| 4 | Hour | BCD |
| 5 | Minute | BCD |
| 6 | Second | BCD |
| 7 | 10 ms counter | Plain hex, `0x00..0x63` |
| 8 | Config byte | Raw byte |

Observed example:

```text
ab000902260306051855443727cf\r\n
```

Which decodes to:

- Date/time: `2026-03-06T18:55:44.550`
- Config byte: `0x27`

Important detail:

- `Capture`: bytes 0..6 are BCD-like date/time fields
- `Capture`: byte 7 is not BCD; it is plain hex in 10 ms units

### 0x09 - CONFIG3 / read mode

`Capture + Spec`

This command controls the reader's message mode and related options.

#### Query form

Observed request:

```text
ab00ff0995\r\n
```

Observed reply:

```text
ab0002090305f3\r\n
```

Reply payload:

| Byte | Meaning |
| --- | --- |
| 0 | CONFIG3 byte |
| 1 | Event timeout in seconds |

#### Set form

Observed set requests use three payload bytes:

| Byte | Meaning |
| --- | --- |
| 0 | CONFIG3 value |
| 1 | Event timeout |
| 2 | Mask of CONFIG3 bits to modify |

Observed examples:

```text
ab00030900050758\r\n  # mode 0x00, timeout 5, mask 0x07
ab0003090305075b\r\n  # mode 0x03, timeout 5, mask 0x07
```

Observed ACK:

```text
ab00000929\r\n
```

Mode bits (`CONFIG3 & 0x07`):

| Value | Meaning | Source |
| --- | --- | --- |
| `0x00` | Normal / raw | Capture + Spec |
| `0x01` | Trigger | Spec |
| `0x02` | Trigger 2 | Spec |
| `0x03` | Event | Capture + Spec |
| `0x04` | Event 2 | Spec |
| `0x05` | First/Last seen | Spec |

Other CONFIG3 bits:

| Bit | Meaning |
| --- | --- |
| 3 | Send status as tag |
| 4 | Sleep on startup |
| 5 | Sleep active low |
| 6 | Sleep tracks RF TX |
| 7 | Listen while RF is off |

Notes:

- `Spec`: the optional third "mask" byte is a later-firmware extension, noted
  as available after controller 8.1 / FPGA 10.2 / HH 4.0
- `Capture`: this reader supports both query mode (`LL = ff`) and the 3-byte
  set form
- `Spec`: command `0x36` also exists as a dedicated "modify part of CONFIG3"
  command, but it was not observed in these captures

### 0x0a - GET_STATISTICS

`Capture + Spec`

Request:

```text
ab00000a51\r\n
```

Observed reply on this reader:

```text
ab000f0af800270000ff31144c2b00034500015b\r\n
```

The 2007 PDF documents parameters 0..13. This 2013 reader appends one extra
byte at the end.

| Byte | Meaning | Confidence |
| --- | --- | --- |
| 0 | Firmware version (`major.minor`, decimal/hex nibble pair) | Spec |
| 1 | Reader ID | Spec |
| 2 | CONFIG1 | Spec |
| 3 | CRC error count | Spec |
| 4 | Power-up count | Spec |
| 5 | Activity count / decoder noise measure | Spec |
| 6 | Decoder I-channel firmware version | Spec |
| 7 | Decoder Q-channel firmware version | Spec |
| 8 | CONFIG2 | Spec |
| 9 | Wiegand config | Spec |
| 10 | Wiegand test timer | Spec |
| 11 | CONFIG3 | Spec |
| 12 | Hardware code | Spec |
| 13 | Filter reject count | Spec |
| 14 | Extra later-firmware byte (`0x01` in all captures) | Capture |

Observed values:

- Firmware version: `0xf8` -> `15.8`
- Reader ID: `0x00`
- CONFIG1: `0x27`
- CONFIG3: `0x03`
- Hardware code: `0x45`
- Extra byte: `0x01`

Notes:

- `Spec`: bytes 3, 4, 5, and 13 are cleared when this message is read
- `Spec`: reply payloads grow by appending new parameters at the end
- `Spec`: `0xff` in the firmware-version field is a special marker for
  single-device combined controller/decoder readers; that was not observed here

### 0x2c - trigger-button / gun-time event

`Capture`

Observed only in `docs/ipico-protocol/captures/guntime.pcapng`:

```text
ab000a2c260306052004151b2782ae\r\n
```

Payload layout:

| Byte | Meaning | Confidence |
| --- | --- | --- |
| 0..8 | Same layout as `0x02` date/time reply | Inference |
| 9 | Extra event byte (`0x82` in the capture) | Unknown |

Decoded timestamp:

- `2026-03-06T20:04:15.270`
- Config byte `0x27`
- Extra byte `0x82`

This capture was taken by pressing the trigger button on the reader, so `0x2c`
can now be treated as the reader's trigger-button / gun-time event. The 2007
PDF still does not document the command, so the field layout remains
undocumented even though the event source is now confirmed.

### 0x30 / 0x31 - tag filter select pattern and mask

#### 0x30 - set/query select pattern

`Capture + Spec`

Observed write:

```text
ab000830058000000000000038\r\n
```

Observed ACK:

```text
ab00003023\r\n
```

`Spec`: `LL = ff` queries the current pattern.

#### 0x31 - set/query select mask

`Capture + Spec`

Observed write:

```text
ab000831bdff000000000000fe\r\n
```

Observed ACK:

```text
ab00003124\r\n
```

`Spec`: `LL = ff` queries the current mask.

#### Match rule

`Spec`

For select filtering:

```text
((observed_tag XOR select_pattern) AND select_mask) == 0
```

If the result is all zeros, the tag matches.

Notes:

- `Spec`: `0x32` and `0x33` are reject-pattern and reject-mask companions
- `Spec`: `0x34` saves filter settings to EEPROM
- `Capture`: only `0x30` and `0x31` were seen on the wire in this repo

### 0x37 - PRINT_STARTUP_BANNER

`Capture + Spec`

Request:

```text
ab0000372a\r\n
```

Behavior:

- Emit the plain ASCII banner text
- Emit an ACK frame with instruction `0x37`

Notes:

- `Spec`: available in controller firmware 8.1 and higher
- `Capture`: observed repeatedly during the TCP bootstrap

### 0x4b - extended status / record management channel

`Capture`

This command is not present in the 2007 PDF. It is a later-firmware command on
this reader and it serves multiple roles:

- Query current extended status
- Toggle recording on/off
- Download stored events from internal memory
- Clear stored records

#### Query form

Observed request:

```text
ab00ff4bc2\r\n
```

Observed payload shapes:

```text
000000000000000059058f0100      # empty idle state
010204f60000000059058f0314      # stored-data idle, before download
030204f600016c0059058f0300      # stored-data download in progress
010204f60204f60059058f0300      # stored-data idle, after download
```

Observed payload layout:

| Byte(s) | Meaning | Confidence |
| --- | --- | --- |
| 0 | Recorder/access state byte | Capture |
| 1..3 | Stored-data extent / end cursor | Inference |
| 4..6 | Download-progress cursor in the same units as bytes 1..3 | Inference |
| 7 | Always `0x00` in these captures | Capture |
| 8..9 | Hardware identifier (`0x5905`) | Inference |
| 10 | Hardware config (`0x8f`) | Inference |
| 11 | Coarse storage-state byte (`0x01` empty, `0x03` with stored reads on this reader) | Capture |
| 12 | Optional flags byte; `0x14` before download, `0x00` after download or in empty state | Inference |

Byte 0 recording/download state values:

| Value | Meaning | Source |
| --- | --- | --- |
| `0x00` | Recording disabled / idle-off state | Inference from record-on-off and clear-records captures |
| `0x01` | Recording-enabled idle state | Inference from record-on-off capture |
| `0x03` | Download/access mode entered by the host software | Inference from download captures |

Observed states:

- Recording off: `000000000000000059058f0100`
- Recording on, empty memory: `010000000000000059058f0100`
- Downloading: `030000000000000059058f0100`
- Stored-data idle, pre-download: `010204f60000000059058f0314`
- Downloading with progress underway: `030204f600016c0059058f0300`
- Stored-data idle, post-download: `010204f60204f60059058f0300`
- Before record clear: `010b012f0000000059058f0c00`
- After record clear: `000000000000000059058f0100`

What changed after clearing records:

- Byte 0 changed from `01` to `00` (recording state reset)
- Bytes 1..3 changed from `0b012f` to `000000`
- Bytes 4..6 remained `000000`
- Byte 11 changed from `0x0c` to `0x01`
- Optional flags byte remained present when the 13-byte form was used

#### Stored-data extent and download-progress hypothesis

`Inference`

The new non-empty captures invalidate the older "bytes 2..3 = unique tag
count" interpretation. In `con-dis-w-4k-reads.pcapng`, the idle stored-data
status is `010204f60000000059058f0314`. In `downloadreads.pcapng`, byte 0 flips
to `0x03` when the download starts, bytes 1..3 stay fixed at `0204f6`, and
bytes 4..6 advance through intermediate values until they also reach `0204f6`.

That strongly suggests:

- Bytes 1..3 encode a stored-data extent / end cursor
- Bytes 4..6 encode current download progress in the same units
- Byte 11 is a coarse empty/non-empty state byte, not a page counter

What remains unknown is the exact unit and endianness of those counters. They
do not yet map cleanly to the UI's `126kb` capacity display.

Byte 12 now also looks more like a flags field than a size field:

- Empty memory query: `...0100`
- Stored reads, pre-download query: `...0314`
- Stored reads, post-download query: `...0300`

That pattern hints at an "undownloaded records present" flag, but that part is
still only an inference.

#### Observed sub-command roles for writes

`Capture + Inference`

The first byte of the `0x4b` write payload behaves like a sub-command selector
in the observed flows:

| Sub-cmd | Payload | Meaning | Source |
| --- | --- | --- | --- |
| `0x00` | `[state]` | Recording-state write: `0x00`=off, `0x01`=on | Inference from record-on-off |
| `0x01` | `[state]` | Access/download state write: `0x00`=stop, `0x01`=start | Inference from download captures |
| `0x02` | (none) | Preparatory step in download workflow | Capture |
| `0x07` | `[byte1, byte2?]` | Download-workflow parameter block / cleanup | Inference from download captures |
| `0xd0` | (none) | Trigger record clear | Capture |

#### Record on/off workflow

`Capture`

Observed in `docs/ipico-protocol/captures/record-on-off.pcapng`. In this
capture, IPICO Connect toggles recording with `0x4b`, then changes CONFIG3:

Record OFF:

```text
C->R  ab00024b000018        # 0x4b sub-cmd 0x00, state=0x00 (recording off)
R->C  ab000c4b00...         # byte 0 = 0x00
C->R  ab00030900050758      # CONFIG3 set mode=0x00 (raw), timeout=5, mask=0x07
R->C  ab00000929            # ACK
C->R  ab00ff0995            # CONFIG3 query
R->C  ab0002090005f0        # confirmed: raw mode, timeout 5
```

Record ON:

```text
C->R  ab00024b000119        # 0x4b sub-cmd 0x00, state=0x01 (recording on)
R->C  ab000c4b01...         # byte 0 = 0x01
C->R  ab0003090305075b      # CONFIG3 set mode=0x03 (event), timeout=5, mask=0x07
R->C  ab00000929            # ACK
C->R  ab00ff0995            # CONFIG3 query
R->C  ab0002090305f3        # confirmed: event mode, timeout 5
```

Notes:

- In this observed workflow, record-on is followed by event mode (CONFIG3 `0x03`)
- In this observed workflow, record-off is followed by raw mode (CONFIG3 `0x00`)
- The CONFIG3 change is sent by the host software, not triggered by the reader
- This means the reader's read mode and recording state are controlled
  independently, but IPICO Connect keeps them in sync

#### Download events workflow

`Capture`

Observed in both `docs/ipico-protocol/captures/download-events.pcapng`
(empty memory) and `docs/ipico-protocol/captures/downloadreads.pcapng`
(stored reads present). The host-side sub-command sequence is the same in both
captures:

```text
C->R  ab00014b02b9          # sub-cmd 0x02: init download
R->C  ab000c4b01...         # status, byte 0 = 0x01

C->R  ab00034b07010586      # sub-cmd 0x07: configure download [0x01, 0x05]
R->C  ab000c4b01...         # status unchanged

C->R  ab00024b01011a        # sub-cmd 0x01: start download [0x01]
R->C  ab000c4b03...         # byte 0 → 0x03 (download/access mode)

# Empty-memory capture: no record payload appears
# Stored-read capture: 4,102 aa frames stream back-to-back

C->R  ab00024b010019        # sub-cmd 0x01: stop download [0x00]
R->C  ab000c4b01...         # byte 0 → 0x01 (back to recording)

C->R  ab00024b07001f        # sub-cmd 0x07: cleanup [0x00]
R->C  ab000c4b01...         # status unchanged

# Normal polling resumes
```

Notes:

- `downloadreads.pcapng` showed that stored records are transferred as ordinary
  `aa` frames, not as a separate binary dump format
- During the stored-read download, periodic `0x4b` replies continued with
  bytes 4..6 advancing toward the same value held in bytes 1..3
- The exported `testreads` file uses the same `aa` line format as the wire
  capture
- The download workflow does not change CONFIG3 mode, unlike record on/off

#### Clear-records workflow

`Capture`

Observed in `docs/ipico-protocol/captures/delete-records.pcapng`:

```text
C->R  ab00024b000018        # sub-cmd 0x00: [0x00] (set recording off)
C->R  ab00024b010019        # sub-cmd 0x01: [0x00]
C->R  ab00014bd0eb          # sub-cmd 0xd0: trigger clear
R->C  90 progress frames    # pages 0x00..0x59
C->R  ab00ff4bc2            # query to confirm
R->C  cleared status        # bytes 1..6 reset to zero, byte 11 = 0x01
```

After the `0xd0` trigger, the reader emits a progress stream of `0x4b` frames:

```text
ab00034bd00059bb\r\n
ab00034bd00159bc\r\n
ab00034bd00259bd\r\n
...
ab00034bd05959c9\r\n
```

That is:

- Byte 0: `0xd0`
- Byte 1: page number
- Byte 2: constant `0x59`

Pages `0x00..0x59` were observed, so 90 progress frames were emitted.

### 0x4c - unsolicited post-settime status

`Capture`

Seen once, immediately after a successful `0x01` write in `docs/ipico-protocol/captures/connect.pcapng`:

```text
ab00094c01555202a8555201f459\r\n
```

This is clearly a real reader-generated message, but its field layout is still
unknown.

### 0xe0 - bootstrap/init probe

`Capture`

Observed request:

```text
ab0000e055\r\n
```

Behavior:

- Sent during full connection bootstrap
- Preceded by a large block of NUL bytes from host to reader
- No clean, direct reply has been identified with high confidence

Treat this as an observed but undocumented initialization probe.

### 0xe2 - unsupported probe

`Capture`

Observed request:

```text
ab0001e206be\r\n
```

Observed reply:

```text
ab0000f258\r\n
```

So, on this reader:

- Command `0xe2` with payload `06` is rejected with `0xf2`
- The management software still probes it during polling, likely because some
  other reader models support it

## Observed Session Behavior

### TCP bootstrap

`Capture`

Across the full-session captures (`connect`, `con-dis`, `settime`,
`delete-records`, `turnon-con-dis`), the host performs roughly this sequence
after opening TCP port `10000`:

1. `0x02` get date/time
2. `0x0a` get statistics
3. Send a large run of NUL bytes
4. `0xe0` init/probe
5. `0x37` print banner
6. `0x4b` query extended status
7. Enter a polling loop

The exact ordering of repeated `0x37`, `0x0a`, and follow-up queries varies
slightly by workflow, but those commands are the recurring bootstrap pieces.

The `turnon-con-dis` capture confirms this is identical after a fresh power-on.

### Polling loop

`Capture`

The steady-state session repeatedly queries:

- `0x02` date/time
- `0x4b` extended status
- `0xe2` probe, which fails with `0xf2`

Some captures also show:

- `0x0a` get statistics
- `0x09` get CONFIG3 / read mode

While polling is active, the reader can also emit unsolicited traffic:

- `aa` tag reports
- `0x2c` trigger-button / gun-time event
- `0x4c` status/event frame

### Set-clock workflow

`Capture`

Observed in `docs/ipico-protocol/captures/settime.pcapng` and
`docs/ipico-protocol/captures/setclock.pcapng`:

1. Host sends `0x01` (SET_DATE_TIME)
2. Reader ACKs
3. Reader may emit unsolicited `0x4c`
4. Host sends `0x02` (GET_DATE_TIME) to verify
5. Reader returns the updated clock

**Timing behaviour** (confirmed in `setclock.pcapng` across 5 SET attempts):
the new second value does **not** take effect at the moment the command is
received. The centisecond counter continues free-running and the SET is applied
at the next cs rollover (cs wraps 99 → 0). A verify read issued within the
same second will still show the pre-SET second value with a continuing cs
counter. Any clock-sync algorithm must account for this by predicting the wall
time at the rollover moment (arrival + time-to-rollover) and rounding to the
nearest second, since the reader will show `S.000` at rollover

### Read-mode workflow

`Capture`

Observed in `docs/ipico-protocol/captures/connect.pcapng`:

1. Host sends `0x09` set raw mode (`0x00`)
2. Reader ACKs
3. Host queries `0x09`
4. Reader returns `0005`
5. Host sends `0x09` set event mode (`0x03`)
6. Reader ACKs
7. Host queries `0x09`
8. Reader returns `0305`

Additional observation from `docs/ipico-protocol/captures/event-read.pcapng`:

9. While the reader still reports `0305`, a live tag capture shows one initial
   `aa` report and then a delayed resend of the same embedded read timestamp
   about 5.5 seconds later
10. This observed wire behavior is closer to timeout-driven follow-up traffic
    than to a strict "entry-only until the tag leaves and re-enters" model

### Record on/off workflow

`Capture`

Observed in `docs/ipico-protocol/captures/record-on-off.pcapng`:

1. Host sends `0x4b [00, state]` to toggle recording (0=off, 1=on)
2. Reader confirms via status byte 0 changing
3. Host sends `0x09` CONFIG3 to set read mode (raw for off, event for on)
4. Host queries `0x09` to verify

See the `0x4b` command reference for full frame details.

### Download events workflow

`Capture`

Observed in `docs/ipico-protocol/captures/download-events.pcapng` and
`docs/ipico-protocol/captures/downloadreads.pcapng`:

1. Host sends `0x4b [02]` to init download
2. Host sends `0x4b [07, 01, 05]` to configure download
3. Host sends `0x4b [01, 01]` to start download (status byte 0 → `0x03`)
4. Empty-memory case: no records stream back
5. Stored-read case: 4,102 `aa` records stream back in the normal live-read format
6. Periodic `0x4b` replies continue during transfer and expose a moving progress field
7. Host sends `0x4b [01, 00]` to stop download (status byte 0 → `0x01`)
8. Host sends `0x4b [07, 00]` to clean up
9. Normal polling resumes

See the `0x4b` command reference for full frame details.

### Clear-records workflow

`Capture`

Observed in `docs/ipico-protocol/captures/delete-records.pcapng`:

1. Host sends `0x4b [00, 00]` to set recording off
2. Host sends `0x4b [01, 00]`
3. Host sends `0x4b [d0]` to trigger clear
4. Reader emits 90 page-progress frames from `0x00` to `0x59`
5. Host queries `0x4b` again and sees cleared counters
6. Host toggles CONFIG3 during the process, including an Event-mode step

See the `0x4b` command reference for full frame details.

## Appendix: PDF-Documented Host Commands Not Seen In These Captures

> These commands come from the 2007 serial PDF only. They are useful
> family-level background, but they are not validated on the 2013 ARM9 TCP
> reader
> captured in this repo. Do not assume a command is supported on this reader
> unless a capture, test artifact, or implementation reference in this repo
> confirms it.

The 2007 PDF defines many additional host-facing instructions. They are part of
what we know about the broader IPICO protocol family, even though they were not
seen on the 2013 TCP captures in this repo.

### 0x03 - 0x0f

| Code | Name | Notes |
| --- | --- | --- |
| `0x03` | Set CONFIG1 | One-byte config word |
| `0x04` | Set reader ID | One-byte reader ID |
| `0x05` | Set RF synth configuration | Synth control lines |
| `0x06` | RF TX on/off | Controls RF TX state |
| `0x07` | Aux output | Controls auxiliary output |
| `0x08` | CRC seed | Two-byte CRC seed |
| `0x0b` | Self test | Returns RTC / config health bits |
| `0x0c` | Bootload controller | Enters bootloader or branches to it |
| `0x0d` | CRC checking options | Configures CRC handling |
| `0x0e` | Beep options | Reader beeper configuration |
| `0x0f` | Wiegand options | Wiegand output config |

### 0x10 - 0x1f

| Code | Name | Notes |
| --- | --- | --- |
| `0x10` | Bootload decoder PIC | Decoder-slave bootload command |
| `0x11` | Set/get tag ID message format | Controls `aa` report layout |
| `0x12` | Set tag configuration | Includes tag baud-rate selection |
| `0x13` | Set nurse tag | Stores a special tag ID |
| `0x14` | Get nurse tag | Returns the stored special tag ID |
| `0x15` | Set RW command/data | Preloads RW command or RW data |
| `0x16` | Transmit RW command | Sends the preloaded RW command |
| `0x17` | Set RW transmission rate | R->T bit-rate control |
| `0x18` | Reset factory defaults | Restores defaults |
| `0x19` | Configure IO pins | Input/output role mapping |
| `0x1a` | Get IO | Reads current IO state |
| `0x1b` | Get IO settings | Reads IO configuration |
| `0x1c` | Set output 1 | Directly drives output 1 |
| `0x1d` | Set fast multiplex times | Delay/mark/space timing |
| `0x1e` | Start multiplexing | Sync/start current pattern |
| `0x1f` | Set sleep | Immediate sleep control |

### 0x20 - 0x2b

| Code | Name | Notes |
| --- | --- | --- |
| `0x20` | RW action command | Starts RW action sequences |
| `0x21` | RW data | RW data payload |
| `0x22` | RW tag match mask | RW-target selection mask |
| `0x23` | RW command stop | Stops RW command engine |
| `0x24` | RW set timeouts | RW timing and retry control |
| `0x25` | RW immediate action | Suspend/resume style commands |
| `0x26` | RW get status | Returns RW engine status |
| `0x27` | Save RW settings | Persists RW configuration |
| `0x28` | Set RW options | RW option bitmask |
| `0x29` | Set TTO options | TTO report configuration |
| `0x2a` | Set expected TTO page count | Limits/report expectations |
| `0x2b` | Get FPGA statistics | FPGA-specific stats |

The PDF does not describe `0x2c`, `0x4b`, `0x4c`, `0xe0`, or `0xe2`. Those are
later-firmware or reader-specific behavior discovered from captures.

### 0x32 - 0x3a

| Code | Name | Notes |
| --- | --- | --- |
| `0x32` | Set/get reject pattern | Companion to `0x30` |
| `0x33` | Set/get reject mask | Companion to `0x31` |
| `0x34` | Save filter settings to EEPROM | Persists filter config |
| `0x35` | Set/get test options | Test-mode controls |
| `0x36` | Modify part of CONFIG3 | Partial CONFIG3 update |
| `0x38` | Get frequency | Test-board specific in the PDF |
| `0x39` | Set hopping timing | Frequency-hopping timing |
| `0x3a` | Set modulate | RF modulation control |

### Debug / I2C commands

| Code | Name | Notes |
| --- | --- | --- |
| `0x40` | Dump memory | ARM/debug function |
| `0x80` | Send last I2C read contents | Returns buffered I2C data |
| `0x81` | Send I2C message | Direct I2C read/write passthrough |

The PDF also contains a separate decoder-side instruction set accessed via I2C.
That material is relevant to the broader IPICO platform, but it was not seen on
the host-facing TCP wire protocol in this repo and is not expanded here.

## Known Mismatches Between the 2007 PDF and the 2013 Reader Captures

- `0x4b`, `0x4c`, `0xe0`, and `0xe2` do not appear in the 2007 PDF
- `0x2c` is not in the PDF summary, but it is present on the wire as the
  trigger-button / gun-time event
- `0x09` query mode and the 3-byte masked write form are later-firmware
  behavior on this reader
- `0x0a` appends an extra byte beyond the 14 documented parameters
- The startup banner in the captures is an ARM9/FPGA banner, not the older PIC
  examples shown in the PDF

## Open Questions

- What exactly does `0xe0` initialize?
- What is the exact schema of the `0x4c` payload?
- In `0x4b`, what are the exact units and endianness of bytes 1..3 and 4..6?
  The new captures strongly suggest "extent" and "progress", but not the
  exact arithmetic.
- In `0x4b`, what exactly do bytes 8, 9, and 10 describe? They still look like
  a hardware / board identifier block, but that is not proven.
- Does `0x4b` byte 12 = `0x14` mean "undownloaded records present"? It dropped
  to `0x00` after the stored-read download completed.
- Where does the 126kb total memory capacity come from? It is not obviously
  encoded in the `0x4b` status bytes. It may be hardcoded in IPICO Connect per
  reader model.
- How do the `0x4b` extent/progress counters map to the UI's `126kb` capacity
  display and to the apparent number of stored reads?
- What does the constant `0x59` represent in the record-clear progress frames?
  It also appears as byte 8 of the status response — possibly total page count?
- What is the purpose of `0x4b` sub-command `0x07` parameters (`[01, 05]` to
  configure, `[00]` to clean up)?
- What is the purpose of the large NUL-byte burst sent during bootstrap?
- Does any real reader firmware in this product line ever emit literal
  `FS` / `LS` tag suffixes, or is that only a local convention adopted by this
  repo?

## Power-On and Power-Off Behavior

`Capture`

Observed in `docs/ipico-protocol/captures/turnon-con-dis.pcapng`:

- After a fresh power-on, the first IPICO control session uses the same
  bootstrap command sequence as the other full-session captures.
- This capture also confirms the fixed 1024-byte NUL preamble before `0xe0`.
- The user-driven disconnect ends with normal TCP teardown; no application-layer
  disconnect command was observed.
- This file does not isolate power-off behavior, because the reader is
  disconnected before power is cut. No additional IPICO control traffic was seen
  after disconnect.

## Practical Guidance for Future Work

- When implementing against this reader model, trust capture-confirmed behavior
  before the 2007 PDF when they disagree
- Keep `0x4b`, `0x4c`, `0xe0`, `0xe2`, and `0x2c` classified as later-firmware
  behavior until newer vendor docs are found
- Treat the PDF as the best source for generic field names, filter semantics,
  CONFIG3 bits, banner modifiers, and the configurable `aa` tag-report format
- Treat repo-local `FS` / `LS` suffix handling as provisional until a real
  capture confirms it; the direct-FSLS capture added here still did not show it
