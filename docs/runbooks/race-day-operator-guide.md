# Race-Day Operator Guide

This guide is for race-day operations using:
- Forwarder SBCs connected to IPICO readers
- Server coordination for endpoint registration and allow-list distribution
- Receiver UI for subscriptions, local TCP outputs, and IPICO Connect feed

Assumptions:
- Server, forwarders, and receiver are already installed.
- Each forwarder has a unique P2P identity and a server bearer token.
- Operator is trained on timing operations.

---

## Start-to-Finish Race-Day Flow

1. Set up each reader + forwarder location.
   - Set up the reader at the timing point.
   - Connect the reader to the network and turn it on.
   - Power on the SBC forwarder.
   - Open the forwarder's local status page and confirm the reader is connected.
   - Confirm the forwarder registers with server and is allowed for the expected receiver endpoint.
   - If needed, edit reader targets in the forwarder's local config page.
   - Repeat this full step for every forwarder you plan to use.

2. Name forwarders clearly.
   - Open each forwarder's local config page.
   - In `General`, set `Display Name` (examples: `Start`, `Split 1`, `Finish`).
   - Save before continuing.

3. Advance stream epochs before official race reads.
   - Open each forwarder's local status page.
   - Advance the current epoch for every active reader stream, giving it a
     name (for example `Race 1`). Names persist across forwarder restarts.
   - If the advance fails, confirm the forwarder is online and retry.
   - Note: advancing the epoch labels new reads; it does NOT stop older reads
     from reaching a receiver. A freshly subscribed receiver replays all
     retained epochs unless its per-stream `From epoch` control is set — set
     it to the new epoch on the receiver if pre-race data must not reach the
     timing software.

4. Start receiver and verify P2P connection.
   - Open Receiver UI.
   - Confirm the server URL and token are configured.
   - Click `Connect`.
   - Receiver selection mode defaults to `manual`.
   - Subscribe to required streams and note the local port for each subscribed stream.
   - In the stream list, confirm each selected stream shows the expected current stream epoch and epoch name when present.
   - Confirm Receiver UI is not marked `(degraded)` and every subscribed stream shows a usable local port.
   - If `(degraded)` appears or a stream has a port collision, set a unique port override for the affected stream and re-check.

5. Connect IPICO Connect to receiver local outputs.
   - In IPICO Connect, add TCP input(s) to `127.0.0.1:<local_port>`.
   - Add one input per subscribed stream as needed.
   - Load participant and chip files in IPICO Connect or your timing software as usual.

6. Run a test read.
   - Pass a test chip.
   - Confirm it appears in Receiver UI and in IPICO Connect.

7. Start race operations.
   - Begin official timing once test read validation is complete.
   - Monitor forwarder reader state, receiver subscription state, and IPICO Connect during the event.

---

## Quick Recovery Checks (During Race)

- No reads in IPICO Connect:
  - Check receiver connection state.
  - Confirm the stream is subscribed.
  - Confirm Receiver UI is not `(degraded)` and the stream does not show a port collision.
  - If there is a collision, set a unique local port override for the affected stream and retry.
  - Confirm IPICO Connect is pointed at the correct local port.

- Stream offline or stale:
  - Check reader power/network.
  - Check the forwarder's local status page.
  - Confirm the forwarder is registered and authorized in server.
  - Verify reader target config in the forwarder's local config page.

- Wrong or mixed race data:
  - Confirm IPICO Connect loaded the intended participant and chip files.
  - Confirm the epoch was advanced for all active streams before race start.
  - Set each subscribed stream's `From epoch` control to the race's epoch so
    older epochs are not fetched (epoch advance alone does not prevent a
    receiver from replaying retained pre-race reads).
  - Confirm Receiver UI is subscribed only to the streams intended for this race.
  - Verify the stream entry shows the expected stream epoch and epoch name for the race in progress.

- Re-send one race's reads to timing software (crash/data-loss recovery):
  - In the Streams tab, set the stream's `From epoch` to the race's epoch.
  - In the Admin tab, run `Reset Stream Data` for that stream (subscription
    and the From-epoch setting are preserved).
  - Reconnect the timing software to the stream's local port; it receives
    only the chosen epoch onward.

- Stream shows `halted` (conflicting data at a seq):
  - The receiver detected a record that conflicts with data it already stored
    at the same sequence number and stopped delivery for safety (this can
    happen after a forwarder journal loss between registry pushes).
  - Do NOT clear receiver data blindly mid-race. Advance the epoch on the
    forwarder, then on the receiver set `From epoch` to the new epoch and run
    `Reset Stream Data` for the affected stream to resume from a clean
    boundary.

- Stream shows `paused: epoch unavailable`:
  - The `From epoch` selection is not (or no longer) advertised by the
    forwarder, so the receiver holds delivery rather than sending older data.
  - Pick an available epoch (or `All available data`) to resume.
