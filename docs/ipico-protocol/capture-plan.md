# IPICO Capture Plan

This is a short, prioritized list of captures that would close the biggest
remaining gaps in `docs/ipico-protocol/ipico-control-protocol.md`.

The main rule is simple: prefer captures driven by the vendor software first,
and prefer read-only or clearly reversible actions before trial-and-error with
our own software.

## Priority Captures

| Capture | Risk | How to trigger it | What it answers |
| --- | --- | --- | --- |
| `fsls-one-tag-enter-exit.pcapng` | Low | Set read mode to First/Last Seen, move one tag into the beam, leave it there, then remove it and wait for timeout | Whether real readers emit literal `FS` / `LS` suffixes, or carry first/last state in TTO bytes as the PDF suggests |
| `tag-format-tto-enabled.pcapng` | Medium | Use the vendor tool, if possible, to enable TTO fields or otherwise change tag-report format | How `0x11` really changes `aa` frames on this firmware |
| `settime-small-large-rollover.pcapng` | Low | Set the clock by a small offset, then a large offset, then across a date rollover | What the unsolicited `0x4c` payload means after `0x01` |
| `trigger-single-hold-double.pcapng` | Low | Press the trigger once, hold it, release it, then press it twice quickly; ideally also try it while tags are being read | Whether `0x2c` is a simple edge event or has richer button semantics |
| `connect-after-power-cycle.pcapng` | Low | Power-cycle the reader, then do a clean connect/disconnect with no other actions | Whether startup changes the NUL burst, `0xe0`, banner flow, or any early status frames |
| `extstatus-empty-vs-nonempty.pcapng` | Low | Capture `0x4b` queries when the reader is empty, after some reads, and after reconnect | Which `0x4b` bytes are unique-count, stored-page, flags, or hardware markers |
| `filter-query-and-reject.pcapng` | Medium | Query and, if exposed by the tool, set select/reject pattern and mask without saving | Whether `0x30`-`0x33` behave exactly like the PDF says on this reader |
| `feature-specific-ops.pcapng` | Medium | If the vendor tool exposes self-test, IO status, Wiegand, or frequency-related features, capture one action per file | Which PDF-documented commands are actually implemented on this firmware |

## Existing Baseline

`docs/ipico-protocol/captures/con-dis.pcapng` already covers a clean connect
and disconnect with no extra user actions. That means the missing baseline is
not "connect with no actions", it is "connect with no actions immediately after
reader startup".

## Safest Order

1. Capture more trigger-button cases.
2. Capture repeated clock-set cases.
3. Capture post-boot connect/disconnect.
4. Capture `0x4b` in known reader states without clearing anything.
5. Capture FSLS mode on a real reader.
6. Capture tag-format changes only if the vendor software exposes them.
7. Try filter-query and reject-pattern features only after the low-risk set is
   complete.

## If We Need To Use Our Software

Start with query-only traffic that the reader already handles in the captures:

- `0x02` get date/time
- `0x09` query CONFIG3 (`LL = ff`)
- `0x0a` get statistics
- `0x4b` query extended status (`LL = ff`)
- `0x30` / `0x31` query current select pattern and mask

Good rule:

- Reproduce a frame sequence already seen from the vendor tool before inventing
  a new one.
- Change one variable per capture.
- Write down exactly what physical action happened during the capture.
- Prefer actions that can be undone by power-cycle or by restoring a known
  setting in the vendor tool.

## Avoid For Now

Until we have a sacrificial reader or a clearly recoverable setup, avoid:

- `0x0c` bootloader entry
- `0x10` bootloading a decoder PIC
- `0x18` factory reset
- `0x27` save RW settings
- `0x34` save filter settings to EEPROM
- `0x80` / `0x81` debug and direct I2C commands
- Unknown write commands
- Fuzzing undocumented commands

Also be careful with:

- `0x4b` clear-record flows, because they are destructive to stored data
- `0x11` format changes, if the vendor tool does not make it easy to restore a
  known-good output format
- Filter writes, because they can make the reader appear "dead" by excluding
  tags rather than by damaging the reader

## Capture Hygiene

- One user action per file is ideal.
- Start the capture before connecting.
- Record whether the reader had just been power-cycled.
- Record whether tags were present in the field.
- Give the file a name that says exactly what changed.

