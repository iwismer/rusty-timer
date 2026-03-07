# IPICO Capture Plan

This is a practical checklist for collecting the next round of IPICO protocol
captures.

It is based on the current gaps in
`docs/ipico-protocol/ipico-control-protocol.md`, plus the extra context that:

- `IPICO Dashboard` is the control-plane tool
- `IPICO Connect` is the live-read path into the timing software

The safest approach is:

- Prefer `IPICO Dashboard` for control/config/status captures
- Prefer `IPICO Connect` for tag-read and timing captures
- Prefer read-only or obviously reversible actions first
- Keep one meaningful action per capture file

## Completed Captures

These have already been collected and analyzed:

| File | What it covers |
| --- | --- |
| `connect.pcapng` | Full TCP bootstrap, status polling, filter writes, clock set, read-mode changes |
| `con-dis.pcapng` | Full bootstrap, steady-state polling, disconnect |
| `settime.pcapng` | Full bootstrap, single clock set + verify |
| `delete-records.pcapng` | Full bootstrap, record-clear sequence via `0x4b` |
| `guntime.pcapng` | Mid-stream trigger-button / gun-time event |
| `read4tags.pcapng` | Mid-stream `aa` tag reports plus concurrent polling traffic |
| `con-dis-w-4k-reads.pcapng` | Dashboard connect/disconnect while stored reads were already present; confirms non-empty idle `0x4b` status |
| `captures/download-events.pcapng` | Download-events workflow via `0x4b` sub-commands (memory was empty) |
| `captures/downloadreads.pcapng` | Dashboard download with stored reads present; shows the records stream back as ordinary `aa` lines |
| `captures/direct-fslsreads-con-dis.pcapng` | Direct FSLS session; only one `aa` frame was captured and it had no literal `FS` / `LS` suffix |
| `captures/direct-raw-reads-con-dis.pcapng` | Direct raw session; `aa` traffic only, with the same 36-character layout seen elsewhere |
| `captures/record-on-off.pcapng` | Record-off then record-on toggle via `0x4b` + CONFIG3 |
| `captures/turnon-con-dis.pcapng` | Full power-on, connect, poll, disconnect; confirms bootstrap after fresh boot but does not isolate power-off |

## Software Roles

Use `IPICO Dashboard` for:

- connect / disconnect
- status polling
- clock sync
- download stored reads
- reader info
- read-mode changes
- filter changes, if exposed

Use `IPICO Connect` for:

- receiving live tag reads from the reader
- passing those reads along to the timing software
- timing behavior
- FSLS behavior, if it respects the current reader mode
- any capture where live reads need to happen while Dashboard is still polling

## General Capture Rules

- Start the packet capture before you connect the app to the reader.
- Stop the capture after the action is complete and you have seen 2-3 more poll
  cycles.
- If you change a setting and then want to observe resulting traffic, split that
  into two files: one for the setting change, one for the resulting behavior.
- If the workflow exports a file, keep that file next to the `.pcapng` with a
  matching base name.
- Write down:
  - which app you used
  - the exact button/setting you changed
  - whether the reader had just been power-cycled
  - whether any tags were in the field

## Recommended Capture Set

Ordered by priority — highest-value gaps first.

### 1. Small known-count stored-read download

Why:

- `downloadreads.pcapng` answered the format question: stored reads come back
  as ordinary `aa` lines
- What is still unknown is how the `0x4b` extent/progress fields map to a known
  number of records and to the UI's capacity display
- A small, counted sample is much easier to reason about than the 4,102-read
  download we already have

Suggested files:

- `download-small-known-count.pcapng`
- `download-small-known-count.txt`

App:

- `IPICO Dashboard`

Steps:

1. Start from empty memory if possible.
2. Generate a small, known number of stored reads, ideally 8-16 total.
3. Write down the exact count you generated and the memory display before
   download.
4. Start capture.
5. Use `IPICO Dashboard` to download the stored reads and save the exported
   file alongside the capture.
6. Wait for the download to complete and a few poll cycles.
7. Stop capture.

Important:

- Keep the exported file so the downloaded `aa` lines can be counted exactly.
- This capture is now about the meaning of the `0x4b` counters, not about the
  download data format itself.

### 2. Repeat download without new reads

Why:

- In `downloadreads.pcapng`, `0x4b` byte 12 changed from `0x14` to `0x00`
  after the download completed
- Bytes 1..3 stayed nonzero, so the reader still thought stored data existed
- We need to know whether that flag means "undownloaded records present"

Suggested files:

- `post-download-status-no-new-reads.pcapng`
- `download-same-buffer-again.pcapng`
- `download-same-buffer-again.txt`

App:

- `IPICO Dashboard`

Steps for `post-download-status-no-new-reads.pcapng`:

1. Leave the reader as-is after a successful download.
2. Do not generate any new reads.
3. Start capture.
4. Reconnect `IPICO Dashboard` and watch steady `0x4b` polling for 10-15
   seconds.
5. Stop capture.

Steps for `download-same-buffer-again.pcapng`:

1. Without generating new reads after the first download, start a new capture.
2. Use `IPICO Dashboard` to run the same download action again.
3. Save the second exported file.
4. Wait for cleanup and a few poll cycles.
5. Stop capture.

### 3. Trigger-button captures

Why:

- Confirms whether `0x2c` is only a press event or has richer button semantics

Suggested files:

- `trigger-single-press.pcapng`
- `trigger-hold-release.pcapng`
- `trigger-double-press.pcapng`
- `trigger-while-reading.pcapng`

App:

- `IPICO Dashboard` connected for polling
- `IPICO Connect` only for the `trigger-while-reading` case

Steps for `trigger-single-press.pcapng`:

1. Start capture.
2. Connect `IPICO Dashboard` to the reader.
3. Wait for steady polling.
4. Press the trigger button once.
5. Wait 5-10 seconds.
6. Stop capture.

Steps for `trigger-hold-release.pcapng`:

1. Start a new capture.
2. Connect `IPICO Dashboard`.
3. Hold the trigger for a noticeable duration, then release it.
4. Wait 5-10 seconds.
5. Stop capture.

Steps for `trigger-double-press.pcapng`:

1. Start a new capture.
2. Connect `IPICO Dashboard`.
3. Press the trigger twice quickly.
4. Wait 5-10 seconds.
5. Stop capture.

Steps for `trigger-while-reading.pcapng`:

1. Start a new capture.
2. Connect `IPICO Dashboard`.
3. Use `IPICO Connect` while presenting tags to the reader so live reads are
   flowing.
4. While reads are happening, press the trigger once.
5. Wait for the reads to finish and for a few more polls.
6. Stop capture.

### 4. Clock-sync captures

Why:

- Helps decode the unsolicited `0x4c` frame that appears after `0x01`

Important constraint:

- `IPICO Dashboard` only syncs the reader to the current computer time

Safe method:

- Temporarily disable automatic time sync on the computer
- Change the host clock by a small amount
- Sync the reader once
- Restore the host clock after the file is captured

Suggested files:

- `clock-sync-normal.pcapng`
- `clock-sync-host-plus-2m.pcapng`

App:

- `IPICO Dashboard`

Steps for `clock-sync-normal.pcapng`:

1. Make sure the computer clock is correct.
2. Start capture.
3. Connect `IPICO Dashboard`.
4. Click the clock-sync action once.
5. Wait for the immediate response and 2-3 poll cycles.
6. Stop capture.

Steps for `clock-sync-host-plus-2m.pcapng`:

1. Disable automatic time sync on the computer.
2. Move the computer clock ahead by about 2 minutes.
3. Start a new capture.
4. Connect `IPICO Dashboard`.
5. Click the clock-sync action once.
6. Wait for 2-3 poll cycles.
7. Stop capture.
8. Restore the computer clock.

Risk note:

- Do not start with huge time jumps or far-future dates.

### 5. FSLS on a real reader

Why:

- `direct-fslsreads-con-dis.pcapng` already argues against a literal `FS` /
  `LS` suffix
- What is still missing is a full enter/hold/exit sequence on a real reader

Suggested files:

- `fsls-set-mode.pcapng`
- `fsls-one-tag-enter-exit.pcapng`

App:

- `IPICO Dashboard` to set the mode
- `IPICO Connect` or the normal timing setup while real reads are happening

Steps for `fsls-set-mode.pcapng`:

1. Start capture.
2. Connect `IPICO Dashboard`.
3. Change the reader to First/Last Seen mode.
4. Wait for the setting to be applied and verified.
5. Stop capture.

Steps for `fsls-one-tag-enter-exit.pcapng`:

1. Start a new capture with the reader already in FSLS mode.
2. Use exactly one tag.
3. Move it into the beam.
4. Leave it in place for a few seconds.
5. Remove it.
6. Wait longer than the configured FSLS timeout.
7. Stop capture.

### 6. Power-off while still connected

Why:

- `turnon-con-dis.pcapng` proves the normal bootstrap after a fresh power-on
- It does not prove what happens when power is cut while a control session is
  still active

Suggested files:

- `power-off-while-connected.pcapng`

App:

- `IPICO Dashboard` or `IPICO Connect`

Steps:

1. Start capture.
2. Connect the app to the reader.
3. Wait for steady polling.
4. Do not click disconnect.
5. Remove power from the reader.
6. Leave the capture running for 10-15 seconds.
7. Stop capture.

Important:

- This is only useful if the TCP session is still open when power is cut.
- Write down whether the app reports timeout, connection reset, or a clean
  close.

### 7. Tag-format / TTO captures

Why:

- Helps decode command `0x11` and the real shape of `aa` reports

Suggested files:

- `tag-format-enable-tto.pcapng`
- `tag-format-tto-one-tag.pcapng`

App:

- Only if `IPICO Dashboard` exposes tag-format or TTO options clearly

Steps for `tag-format-enable-tto.pcapng`:

1. Start capture.
2. Connect `IPICO Dashboard`.
3. Change only one format option, ideally enabling TTO fields.
4. Wait for the setting write and any verification queries.
5. Stop capture.

Steps for `tag-format-tto-one-tag.pcapng`:

1. Start a new capture with the new format already active.
2. Generate one simple tag read.
3. Wait for the resulting traffic to settle.
4. Stop capture.

### 8. Filter-query and reject-pattern captures

Why:

- Helps confirm `0x30`-`0x33`

Suggested files:

- `filter-query-only.pcapng`
- `filter-write-no-save.pcapng` (only if the UI makes recovery obvious)

App:

- `IPICO Dashboard`, if it exposes filter controls

Steps for `filter-query-only.pcapng`:

1. Start capture.
2. Connect `IPICO Dashboard`.
3. Open any filter/status screen that causes the app to query current pattern
   and mask.
4. Wait for the replies.
5. Stop capture.

Steps for `filter-write-no-save.pcapng`:

1. Only do this if the UI makes it easy to restore a known-open filter.
2. Start a new capture.
3. Apply one reversible filter change.
4. Wait for write and verification traffic.
5. If possible, restore the original filter before stopping.
6. Stop capture.

### 9. Feature-specific captures

Why:

- Any extra buttons in Dashboard may expose commands not yet seen

Suggested files:

- `selftest.pcapng`
- `io-status.pcapng`
- `wiegand-config.pcapng`
- `frequency-info.pcapng`

App:

- `IPICO Dashboard`, only if those features are clearly exposed

Steps:

1. Start capture.
2. Connect `IPICO Dashboard`.
3. Trigger exactly one feature or button.
4. Wait for the response.
5. Stop capture.

## Priority Order

If time is limited, start with these:

1. `download-small-known-count.pcapng` — best shot at mapping `0x4b`
   extent/progress counters to a known number of stored reads
2. `post-download-status-no-new-reads.pcapng` +
   `download-same-buffer-again.pcapng` — tests whether byte 12 means
   "undownloaded records present"
3. `trigger-single-press.pcapng` — confirms `0x2c` semantics
4. `trigger-hold-release.pcapng` — checks for hold/release events
5. `clock-sync-normal.pcapng` — helps decode `0x4c`
6. `fsls-one-tag-enter-exit.pcapng` — settles real FSLS behavior on the wire
7. `power-off-while-connected.pcapng` — isolates shutdown behavior while the
   socket is still open

Then, if things are going smoothly:

8. `fsls-set-mode.pcapng`
9. `clock-sync-host-plus-2m.pcapng`
10. Remaining trigger captures

## No Longer Needed

These were previously planned but are now covered:

- `download-with-records.pcapng` — covered by `downloadreads.pcapng`
- `connect-after-power-cycle.pcapng` — covered by `turnon-con-dis.pcapng`
- `extstatus-empty.pcapng` — we have many empty-state `0x4b` polls across
  multiple captures
- `extstatus-after-reconnect.pcapng` — `turnon-con-dis.pcapng` and
  `con-dis.pcapng` together cover this
- `extstatus-after-reads.pcapng` — covered by `con-dis-w-4k-reads.pcapng` and
  `downloadreads.pcapng`

Still useful if shutdown behavior matters:

- `power-off-while-connected.pcapng` — `turnon-con-dis.pcapng` disconnected
  before power was cut, so it did not isolate power-off behavior

## If We Need To Use Our Software

Start with query-only traffic that the reader already handles in the existing
captures:

- `0x02` get date/time
- `0x09` query CONFIG3 (`LL = ff`)
- `0x0a` get statistics
- `0x4b` query extended status (`LL = ff`)
- `0x30` / `0x31` query current select pattern and mask

Good rule:

- Reproduce a frame sequence already seen from the vendor tool before inventing
  a new one.
- Change one variable per capture.
- Prefer actions that can be undone by power-cycle or by restoring a known
  setting in Dashboard.

## Avoid For Now

Until there is a sacrificial reader or a clearly recoverable setup, avoid:

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
- `0x11` format changes, if the UI does not make it easy to restore a known
  format
- Filter writes, because they can make the reader appear "dead" by excluding
  tags rather than by damaging the reader
