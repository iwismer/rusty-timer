import type { HelpContext } from "./help-types";

export const RECEIVER_HELP = {
  config: {
    title: "Receiver Configuration",
    overview: "Core connection settings for the receiver. These determine how the receiver identifies itself and connects to the remote server.",
    fields: {
      receiver_id: {
        label: "Receiver ID",
        summary: "Unique identifier for this receiver instance.",
        detail: "A unique string that identifies this receiver to the server. The server uses this ID to track which receiver is connected and manage stream subscriptions. Use a descriptive ID like 'finish-line-pc' or 'timing-tent-a' so operators can identify which receiver is which.",
        default: "None (required)",
        recommended: "Use a short, descriptive name that identifies the physical location or purpose of this receiver.",
      },
      server_url: {
        label: "Server URL",
        summary: "WebSocket URL of the remote timing server.",
        detail: "The full WebSocket URL of the server to connect to (e.g. wss://server.example.com:8080/ws/receivers). The receiver maintains a persistent WebSocket connection to the server for real-time data streaming. Use wss:// for encrypted connections.",
        default: "None (required)",
        recommended: "Use wss:// for production to encrypt data in transit.",
      },
      token: {
        label: "Token",
        summary: "Authentication token for connecting to the server.",
        detail: "The authentication token used to authenticate this receiver with the server. The token is sent during the WebSocket handshake. It must match a valid token configured on the server.",
        default: "None (required for authenticated servers)",
      },
      update_mode: {
        label: "Update Mode",
        summary: "How the receiver checks for and applies software updates.",
        detail: "Controls the receiver's update behavior.\n\n<strong>Automatic</strong>: Checks for updates and downloads/applies them automatically. The application will restart to apply.\n\n<strong>Check Only</strong>: Checks for updates and notifies but does not download or apply them.\n\n<strong>Disabled</strong>: No update checking. Use this on race day to prevent unexpected restarts.",
        default: "Automatic",
        range: "Automatic, Check Only, Disabled",
        recommended: "Set to Disabled on race day to prevent unexpected restarts.",
      },
    },
    tips: [
      "Save your config before connecting for the first time.",
      "Set Update Mode to Disabled on race day to prevent unexpected application restarts.",
      "If the receiver can't connect, verify the Server URL is correct and the server is reachable from this machine.",
    ],
    seeAlso: [{ sectionKey: "receiver_mode", label: "Receiver Mode" }],
  },
  receiver_mode: {
    title: "Receiver Mode",
    overview: "The receiver mode determines how streams are subscribed and how epoch controls behave. Choose the mode that matches your timing workflow.",
    fields: {
      mode: {
        label: "Mode",
        summary: "Operating mode: Live, Race, or Targeted Replay.",
        detail: "The receiver supports three operating modes:\n\n<strong>Live</strong>: Auto-subscribes to all available streams from the server. New streams are automatically added as forwarders connect. Earliest-epoch overrides can be set per stream to skip historical data. This is the default mode for standard race timing.\n\n<strong>Race</strong>: Follows server-defined stream assignments based on a selected race configuration. The server determines which streams belong to the race. Epoch controls are shown but disabled since the race configuration controls them. Use this when the server operator has set up race definitions.\n\n<strong>Targeted Replay</strong>: Allows per-stream epoch selection for replaying historical data. Each stream can be configured to replay from a specific epoch. Use this to re-send historical timing data to your timing software, for example to recover from a timing software crash.",
        default: "Live",
        range: "Live, Race, Targeted Replay",
        recommended: "Use Live mode for standard race timing. Switch to Targeted Replay only when you need to re-send historical data.",
      },
      race: {
        label: "Race",
        summary: "Select a race configuration (Race mode only).",
        detail: "When in Race mode, select the race that this receiver should follow. The server provides the list of configured races. The race definition determines which streams are assigned and their epoch settings.",
      },
    },
    tips: [
      "Use Live mode for standard race timing. It auto-subscribes to all available streams.",
      "Switch to Targeted Replay to re-send historical data to your timing software after a crash or data loss.",
      "In Race mode, stream assignments are managed by the server operator. Contact them if streams are missing.",
      "Changing modes takes effect immediately. Active subscriptions may change.",
    ],
    seeAlso: [
      { sectionKey: "streams", label: "Available Streams" },
      { sectionKey: "config", label: "Receiver Configuration" },
    ],
  },
  streams: {
    title: "Available Streams",
    overview: "Streams represent data feeds from forwarder+reader pairs. Each stream delivers chip reads from one reader on one forwarder to the receiver for local forwarding to timing software.",
    fields: {
      stream_identity: {
        label: "Stream",
        summary: "A stream is identified by its forwarder ID and reader IP address.",
        detail: "Each stream represents a unique data feed from a specific reader on a specific forwarder. Streams are identified by the combination of forwarder ID and reader IP. The display alias (if set) shows the forwarder's friendly name instead of the raw ID.",
      },
      subscribed: {
        label: "Subscribed",
        summary: "Whether the receiver is actively receiving data from this stream.",
        detail: "A subscribed stream actively delivers chip reads to the receiver's local port. Unsubscribing a stream stops local delivery but does NOT stop the forwarder from sending data to the server. Data continues to accumulate on the server and can be replayed later.",
      },
      local_port: {
        label: "Local Port",
        summary: "The local TCP port where reads from this stream are forwarded.",
        detail: "The TCP port on the local machine where the receiver forwards chip reads from this stream. Your timing software should be configured to listen on this port. The default port is calculated as 10000 + the last octet of the reader's IP address. Custom ports can be set via the Admin > Port Overrides page.",
      },
      stream_epoch: {
        label: "Epoch",
        summary: "The current epoch (data segment) the stream is reading from.",
        detail: "An epoch represents a segment of timing data on the server, typically corresponding to a race or timing session. Epochs are numbered sequentially. The stream epoch shows which data segment is currently being delivered. Epoch names (if set) provide human-readable labels.",
      },
      earliest_epoch: {
        label: "Earliest Epoch Override",
        summary: "Skip historical epochs and start receiving from a specific epoch.",
        detail: "In Live mode, you can set an earliest-epoch override to skip older data and only receive reads from a specific epoch onward. This is useful when you only care about the current race and don't want to replay historical data. Clear the override to receive all available epochs.",
      },
    },
    tips: [
      "Unsubscribing a stream only stops local delivery. Data continues on the server and can be replayed later.",
      "If your timing software isn't receiving reads, check that it's listening on the correct local port.",
      "The 'degraded' indicator means the server reported an issue with stream data. Reads may still flow but check server logs.",
      "Use Admin > Port Overrides to customize which local port each stream uses if the defaults don't match your timing software setup.",
    ],
    seeAlso: [{ sectionKey: "receiver_mode", label: "Receiver Mode" }],
  },
} as const satisfies HelpContext;
