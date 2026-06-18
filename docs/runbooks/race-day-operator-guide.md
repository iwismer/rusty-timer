# Race-Day Operator Guide

This guide is for race-day operations using:
- Forwarder SBCs connected to IPICO readers
- Thin-node coordination for endpoint registration and allow-list distribution
- Receiver UI for subscriptions, local TCP outputs, and IPICO Connect feed

Assumptions:
- Thin-node, forwarders, and receiver are already installed.
- Each forwarder has a unique P2P identity and a thin-node bearer token.
- Operator is trained on timing operations.

---

## Start-to-Finish Race-Day Flow

1. Set up each reader + forwarder location.
   - Set up the reader at the timing point.
   - Connect the reader to the network and turn it on.
   - Power on the SBC forwarder.
   - Open the forwarder's local status page and confirm the reader is connected.
   - Confirm the forwarder registers with thin-node and is allowed for the expected receiver endpoint.
   - If needed, edit reader targets in the forwarder's local config page.
   - Repeat this full step for every forwarder you plan to use.

2. Name forwarders clearly.
   - Open each forwarder's local config page.
   - In `General`, set `Display Name` (examples: `Start`, `Split 1`, `Finish`).
   - Save before continuing.

3. Reset stream epochs before official race reads.
   - Open each forwarder's local status page.
   - Reset the current epoch for every active reader stream.
   - If reset fails, confirm the forwarder is online and retry.

4. Start receiver and verify P2P connection.
   - Open Receiver UI.
   - Confirm the thin-node URL and token are configured.
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
  - Confirm the forwarder is registered and authorized in thin-node.
  - Verify reader target config in the forwarder's local config page.

- Wrong or mixed race data:
  - Confirm IPICO Connect loaded the intended participant and chip files.
  - Confirm epoch reset was done for all active streams before race start.
  - Confirm Receiver UI is subscribed only to the streams intended for this race.
  - Verify the stream entry shows the expected stream epoch and epoch name for the race in progress.

- Replay requested for one stream but multiple streams replay:
  - Use targeted replay for a single selected stream context.
  - Confirm the selected stream matches the intended P2P stream identity before replaying.
