# Receiver Quickstart (Windows)

The receiver runs on your timing computer and bridges remote timing
streams to local TCP ports so your timing software (e.g. IPICO Connect)
sees the data as if the reader were plugged in directly.

## Download

Download the latest `receiver-*-x86_64-pc-windows-msvc.zip` from the
[Releases](https://github.com/iwismer/rusty-timer/releases) page.

Extract the archive. You should have `rt-receiver.exe`.

## First Run

Double-click `rt-receiver.exe` or run from PowerShell:

```powershell
.\rt-receiver.exe
```

The receiver opens a web UI in your browser. If it doesn't open
automatically, go to **http://localhost:9090**.

## Configure

1. In the receiver UI, enter the **Server URL** (e.g.
   `ws://timing.example.com:8080`) and the **auth token** provided by
   your server operator.
2. Click **Save**, then **Connect**.
3. Once connected, the stream list shows available timing streams.
4. Subscribe to the streams you need. Each subscribed stream gets a
   local TCP port (shown in the UI).

## Connect Timing Software

In IPICO Connect (or your timing software), add a TCP input pointing
at `127.0.0.1` on the local port shown for each subscribed stream.

For example, if the receiver shows port `10100` for a stream, add a
TCP input to `127.0.0.1:10100` in IPICO Connect.

## Port Assignment

By default, each stream's local port is `10000 + last_octet(reader_ip)`.
For a reader at `192.168.1.100`, the local port is `10100`.

If two streams would get the same port (same last octet), set a manual
port override in the receiver UI.

## Data Storage

The receiver stores its configuration and cursor state in:

```
%LOCALAPPDATA%\rusty-timer\receiver\receiver.sqlite3
```

This file is created automatically on first run. If you need to start
fresh, delete this file and restart the receiver.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| Can't connect to server | Check the server URL and token. Ensure the server is reachable from this machine. |
| No reads appearing | Verify the stream is subscribed in the receiver UI and that the forwarder is online in the server dashboard. |
| Port collision warning | Two streams have the same default port. Set a manual port override for one of them. |
| Receiver shows "degraded" | One or more streams have a port conflict. Resolve the conflict in the UI. |

For full operational procedures, see the
[receiver operations runbook](runbooks/receiver-operations.md).
