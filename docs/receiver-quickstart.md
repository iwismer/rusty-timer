# Receiver Quickstart (Windows)

The receiver runs on your timing computer and bridges remote timing streams to
local TCP ports so timing software such as IPICO Connect sees the data as if the
reader were plugged in directly.

## Download

Download the latest `Rusty-Timer-Receiver_*_x64-setup.exe` from the
[Releases](https://github.com/iwismer/rusty-timer/releases) page.

Run the installer. It installs the app and downloads WebView2 if needed. Launch
"Rusty Timer Receiver" from the Start Menu.

## Configure

1. Enter the server URL and receiver token provided by your event operator.
2. Click **Save**, then **Connect**.
3. Once connected, the receiver discovers allowed forwarders and streams.
4. Subscribe to the streams you need by `forwarder_endpoint_id` and `stream_id`.
5. Each subscribed stream gets a local TCP port shown in the UI.

## Connect Timing Software

In IPICO Connect or your timing software, add a TCP input pointing at
`127.0.0.1` on the local port shown for each subscribed stream.

For example, if the receiver shows port `10100` for a stream, add a TCP input to
`127.0.0.1:10100`.

## Port Assignment

When reader IP metadata is available, the default local port is
`10000 + last_octet(reader_ip)`. For a reader at `192.168.1.100`, the local port
is `10100`.

If two streams would get the same port, set a manual port override in the
receiver UI.

## Data Storage

The receiver stores configuration, subscriptions, received events, cursors, and
gap markers in:

```text
%LOCALAPPDATA%\rusty-timer\receiver\receiver.sqlite3
```

This file is created automatically on first run. Deleting it starts fresh and
forgets durable cursors and received-event replay state.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| Cannot connect | Check the server URL, token, and allow-list entry for this receiver endpoint. |
| No reads appearing | Verify the stream is subscribed and the forwarder endpoint is online on the server status board. |
| Gap marker appears | The requested cursor was pruned on the forwarder. Note the gap and resume from the supplied cursor. |
| Port collision warning | Two streams have the same default port. Set a manual port override for one of them. |

### Desktop app exits immediately or shows no window

If the app fails to start, check for a crash log at:

```text
%LOCALAPPDATA%\com.rusty-timer.receiver\crash.log
```

Typical causes include failure to create the WebView2 window or SQLite database
corruption. More detail: [Receiver Tauri development guide](receiver-tauri-dev.md#troubleshooting).

For full operational procedures, see the [receiver operations runbook](runbooks/receiver-operations.md).
