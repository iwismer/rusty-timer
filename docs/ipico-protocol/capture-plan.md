# IPICO Capture Plan

This is a practical checklist for collecting the next round of IPICO protocol
captures.

It is based on the current gaps in
`docs/ipico-protocol/ipico-control-protocol.md`, plus the extra context that:

- `IPICO Dashboard` is the control-plane tool
- `IPICO Connect` is the read/timing tool
- `docs/ipico-protocol/captures/con-dis.pcapng` already covers a clean
  connect/disconnect that was not immediately after reader startup

The safest approach is:

- Prefer `IPICO Dashboard` for control/config/status captures
- Prefer `IPICO Connect` for tag-read and timing captures
- Prefer read-only or obviously reversible actions first
- Keep one meaningful action per capture file

## Software Roles

Use `IPICO Dashboard` for:

- connect / disconnect
- status polling
- clock sync
- reader info
- read-mode changes
- filter changes, if exposed

Use `IPICO Connect` for:

- generating real tag reads
- timing behavior
- FSLS behavior, if it respects the current reader mode
- any capture where active reads need to happen while Dashboard is still polling

## General Capture Rules

- Start the packet capture before you connect the app to the reader.
- Stop the capture after the action is complete and you have seen 2-3 more poll
  cycles.
- If you change a setting and then want to observe resulting traffic, split that
  into two files: one for the setting change, one for the resulting behavior.
- Write down:
  - which app you used
  - the exact button/setting you changed
  - whether the reader had just been power-cycled
  - whether any tags were in the field

## Recommended Capture Set

These are the captures worth trying tomorrow, in the safest order.

### 1. Trigger-button captures

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

Separate the capture:

- Stop after the single press and the following poll traffic.
- Start a new file for the next trigger pattern.

Steps for `trigger-hold-release.pcapng`:

1. Start a new capture.
2. Connect `IPICO Dashboard`.
3. Hold the trigger for a noticeable duration, then release it.
4. Wait 5-10 seconds.
5. Stop capture.

Separate the capture:

- Do not combine this with the single-press case.

Steps for `trigger-double-press.pcapng`:

1. Start a new capture.
2. Connect `IPICO Dashboard`.
3. Press the trigger twice quickly.
4. Wait 5-10 seconds.
5. Stop capture.

Separate the capture:

- Keep this as its own file.

Steps for `trigger-while-reading.pcapng`:

1. Start a new capture.
2. Connect `IPICO Dashboard`.
3. Use `IPICO Connect` to create active tag reads.
4. While reads are happening, press the trigger once.
5. Wait for the reads to finish and for a few more polls.
6. Stop capture.

Separate the capture:

- Keep this separate from the no-tag trigger captures.

### 2. Clock-sync captures

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
- `clock-sync-before-midnight.pcapng` (optional)

App:

- `IPICO Dashboard`

Steps for `clock-sync-normal.pcapng`:

1. Make sure the computer clock is correct.
2. Start capture.
3. Connect `IPICO Dashboard`.
4. Click the clock-sync action once.
5. Wait for the immediate response and 2-3 poll cycles.
6. Stop capture.

Separate the capture:

- One file for the normal sync only.

Steps for `clock-sync-host-plus-2m.pcapng`:

1. Disable automatic time sync on the computer.
2. Move the computer clock ahead by about 2 minutes.
3. Start a new capture.
4. Connect `IPICO Dashboard`.
5. Click the clock-sync action once.
6. Wait for 2-3 poll cycles.
7. Stop capture.
8. Restore the computer clock.

Separate the capture:

- Do not combine this with the normal-sync file.

Steps for `clock-sync-before-midnight.pcapng`:

1. Disable automatic time sync on the computer.
2. Set the computer clock to just before midnight.
3. Start a new capture.
4. Connect `IPICO Dashboard`.
5. Click the clock-sync action once.
6. Wait for the read-back and a few polls.
7. Stop capture.
8. Restore the computer clock.

Separate the capture:

- Keep the date-rollover case in its own file.

Risk note:

- Do not start with huge time jumps or far-future dates.

### 3. Post-boot connect/disconnect capture

Why:

- `con-dis.pcapng` already gives a clean connect/disconnect
- What is still missing is the same flow immediately after reader startup

Suggested file:

- `connect-after-power-cycle.pcapng`

App:

- `IPICO Dashboard`

Steps:

1. Close both IPICO apps.
2. Start packet capture.
3. Power-cycle the reader.
4. Wait for the reader to come back on the network.
5. Open `IPICO Dashboard` and connect to the reader.
6. Do nothing else.
7. Let it poll briefly.
8. Disconnect.
9. Stop capture.

Separate the capture:

- Do not add any clock changes, trigger presses, or reads to this file.

### 4. `0x4b` extended-status captures

Why:

- `0x4b` is the reader's extended-status query
- On the wire it is the host sending `ab00ff4bc2`
- You do not need to craft it manually; Dashboard already polls it

Suggested files:

- `extstatus-empty.pcapng`
- `extstatus-after-reads.pcapng`
- `extstatus-after-reconnect.pcapng`

App:

- `IPICO Dashboard`
- `IPICO Connect` only for generating reads between Dashboard polls

Steps for `extstatus-empty.pcapng`:

1. Make sure no tags are being actively read.
2. Start capture.
3. Connect `IPICO Dashboard`.
4. Let it poll for 10-15 seconds.
5. Stop capture.

Separate the capture:

- This file should contain only the baseline empty-state polling.

Steps for `extstatus-after-reads.pcapng`:

1. Start a new capture.
2. Connect `IPICO Dashboard`.
3. Use `IPICO Connect` to generate a small number of real reads.
4. Leave `IPICO Dashboard` connected for another 10-15 seconds.
5. Stop capture.

Separate the capture:

- This file should contain the transition from "reads just happened" into
  normal polling.

Steps for `extstatus-after-reconnect.pcapng`:

1. Without power-cycling the reader, start a new capture.
2. Connect `IPICO Dashboard`.
3. Let it poll briefly.
4. Disconnect and reconnect Dashboard once.
5. Let it poll again.
6. Stop capture.

Separate the capture:

- Keep reconnect behavior separate from the post-boot connect file.

### 5. FSLS on a real reader

Why:

- This is the cleanest way to settle what real FSLS output looks like

Suggested files:

- `fsls-set-mode.pcapng`
- `fsls-one-tag-enter-exit.pcapng`

App:

- `IPICO Dashboard` to set the mode
- `IPICO Connect` or normal reader operation to generate reads

Steps for `fsls-set-mode.pcapng`:

1. Start capture.
2. Connect `IPICO Dashboard`.
3. Change the reader to First/Last Seen mode.
4. Wait for the setting to be applied and verified.
5. Stop capture.

Separate the capture:

- Stop once the mode change is complete.
- Start a new file for the read behavior itself.

Steps for `fsls-one-tag-enter-exit.pcapng`:

1. Start a new capture with the reader already in FSLS mode.
2. Use exactly one tag.
3. Move it into the beam.
4. Leave it in place for a few seconds.
5. Remove it.
6. Wait longer than the configured FSLS timeout.
7. Stop capture.

Separate the capture:

- Keep this file focused on one tag and one clean enter/leave cycle.

### 6. Tag-format / TTO captures

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

Separate the capture:

- One file per format change.
- Do not combine multiple format toggles in one file.

Steps for `tag-format-tto-one-tag.pcapng`:

1. Start a new capture with the new format already active.
2. Generate one simple tag read.
3. Wait for the resulting traffic to settle.
4. Stop capture.

Separate the capture:

- Keep the format-change capture separate from the resulting tag-traffic capture.

### 7. Filter-query and reject-pattern captures

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

Separate the capture:

- Keep query-only traffic separate from any writes.

Steps for `filter-write-no-save.pcapng`:

1. Only do this if the UI makes it easy to restore a known-open filter.
2. Start a new capture.
3. Apply one reversible filter change.
4. Wait for write and verification traffic.
5. If possible, restore the original filter before stopping.
6. Stop capture.

Separate the capture:

- One file per write action.
- Do not use save-to-EEPROM.

### 8. Feature-specific captures

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

Separate the capture:

- One feature per file.

## Tomorrow's Best First Pass

If time is limited, start with these five:

1. `trigger-single-press.pcapng`
2. `trigger-hold-release.pcapng`
3. `clock-sync-normal.pcapng`
4. `clock-sync-host-plus-2m.pcapng`
5. `connect-after-power-cycle.pcapng`

Then, if things are going smoothly:

6. `extstatus-empty.pcapng`
7. `extstatus-after-reads.pcapng`
8. `extstatus-after-reconnect.pcapng`

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
