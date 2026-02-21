# Race-Day Operator Guide

This guide is for race-day operations using:
- Forwarder (SBC + reader)
- Server UI
- Receiver (for IPICO Connect feed)

Assumptions:
- Deployment is already complete.
- Forwarders, server, and receiver are already installed.
- Operator is trained on timing operations.

---

## Start-to-Finish Race-Day Flow

1. Set up each reader + forwarder location.
  - Set up the reader at the timing point.
  - Connect the reader to the network and turn it on.
  - Power on the SBC forwarder.
  - In Server UI (`Streams`), confirm that stream(s) from that forwarder appear and are online.
  - If needed, edit the reader targets in the forwarder config (`Configure` from Server UI, or forwarder config page directly).
  - Repeat this full step for every forwarder you plan to use.

2. Name forwarders clearly.
  - In Server UI `Streams`, open `Configure` for each forwarder.
  - In `General`, set `Display Name` (examples: `Start`, `Split 1`, `Finish`).
  - Save before continuing.

3. Create the race in Server UI.
  - Open `Races`.
  - Create the race with the event name you want operators to use all day.

4. Import participants and chip assignments.
  - Open the race detail page.
  - Upload the `.ppl` file.
  - Upload the `.bibchip` file.
  - Check participant/chip counts and review unmatched chips.
  - Note: uploading participants or chips replaces the existing participants or chips for that race.

5. Assign the race to each forwarder.
  - Return to `Streams`.
  - For each forwarder group, select the active race from the race dropdown.

6. Reset epoch before official race reads.
  - Open each active stream detail page.
  - Click `Reset Epoch`.
  - Do this for every stream used for the race.
  - If reset fails, confirm the forwarder is online and retry.

7. Start receiver and verify connection.
  - Open Receiver UI.
  - Click `Connect`.
  - Normally, server URL and token are already configured.
  - Only if connection does not work: verify/fix server URL and token, save, then connect again.
  - Subscribe to required streams and note the local port for each subscribed stream.
  - Confirm Receiver UI is not marked `(degraded)` and every subscribed stream shows a usable local port.
  - If `(degraded)` appears or a stream has a port collision, set a unique port override for the affected stream and re-check.

8. Connect IPICO Connect to receiver local outputs.
  - In IPICO Connect, add TCP input(s) to `127.0.0.1:<local_port>`.
  - Add one input per subscribed stream as needed.

9. Run a test read.
  - Pass a test chip.
  - Confirm it appears in Server UI and in IPICO Connect.

10. Start race operations.
  - Begin official timing once test read validation is complete.
  - Monitor stream status and receiver connection during the event.

---

## Quick Recovery Checks (During Race)

- No reads in IPICO Connect:
  - Check receiver connection state.
  - Confirm the stream is subscribed.
  - Confirm Receiver UI is not `(degraded)` and the stream does not show a port collision.
  - If there is a collision, set a unique local port override and retry.
  - Confirm IPICO Connect is pointed at the correct local port.

- Stream offline in Server UI:
  - Check reader power/network.
  - Check forwarder is online.
  - Verify reader target config in forwarder config.

- Wrong or mixed race data:
  - Confirm correct race assignment per forwarder in `Streams`.
  - Confirm epoch reset was done for all active streams before race start.
