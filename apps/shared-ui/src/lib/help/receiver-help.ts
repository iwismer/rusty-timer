import type { HelpContext } from "./help-types";

export const RECEIVER_HELP = {
  config: {
    title: "Receiver Configuration",
    overview: "Core connection settings for the receiver. These determine how the receiver identifies itself and connects to the remote server.",
    fields: {
      receiver_id: {
        label: "Receiver ID",
        summary: "Unique identifier for this receiver instance.",
        detailHtml: "A unique string that identifies this receiver to the server. Use a descriptive ID like 'finish-line-pc' or 'timing-tent-a' so operators can identify which receiver is which.",
        default: "None (required)",
        recommended: "Use a short, descriptive name that identifies the physical location or purpose of this receiver.",
      },
      server_url: {
        label: "Server URL",
        summary: "URL of the remote timing server.",
        detailHtml: "The full URL of the server to connect to (e.g. <code>wss://server.example.com:8080/ws/receivers</code>). The receiver maintains a persistent connection to the server for real-time data streaming. Use <code>wss://</code> for encrypted connections.",
        default: "None (required)",
        recommended: "Use wss:// for production to encrypt data in transit.",
      },
      token: {
        label: "Token",
        summary: "Authentication token for connecting to the server.",
        detailHtml: "The authentication token used to connect this receiver to the server. It must match a valid token configured on the server.",
        default: "None (required for authenticated servers)",
      },
      update_mode: {
        label: "Update Mode",
        summary: "How the receiver checks for and applies software updates.",
        detailHtml: "Controls the receiver's update behavior.<br><br><strong>Automatic</strong>: Checks for updates and downloads/applies them automatically. The application will restart to apply.<br><br><strong>Check Only</strong>: Checks for updates and notifies but does not download or apply them.<br><br><strong>Disabled</strong>: No update checking. Use this on race day to prevent unexpected restarts.",
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
        detailHtml: "The receiver supports three operating modes:<br><br><strong>Live</strong>: Auto-subscribes to all available streams from the server. New streams are automatically added as forwarders connect. You can set earliest-epoch overrides per stream to skip historical data. This is the default mode for standard race timing.<br><br><strong>Race</strong>: Follows server-defined stream assignments based on a selected race configuration. The server determines which streams belong to the race. Use this when the server operator has set up race definitions.<br><br><strong>Targeted Replay</strong>: Allows per-stream epoch selection for replaying historical data. Use this to re-send timing data to your timing software, for example to recover from a timing software crash.",
        default: "Live",
        range: "Live, Race, Targeted Replay",
        recommended: "Use Live mode for standard race timing. Switch to Targeted Replay only when you need to re-send historical data.",
      },
      race: {
        label: "Race",
        summary: "Select a race configuration (Race mode only).",
        detailHtml: "When in Race mode, select the race that this receiver should follow. The server provides the list of configured races. The race definition determines which streams are assigned and their epoch settings.",
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
    overview: "Streams represent data feeds from forwarder/reader pairs. Each stream delivers chip reads from one reader to the receiver, which forwards them to your timing software.",
    fields: {
      stream_identity: {
        label: "Stream",
        summary: "A stream is identified by its forwarder ID and reader IP address.",
        detailHtml: "Each stream represents a unique data feed from a specific reader on a specific forwarder. Streams are identified by the combination of forwarder ID and reader IP. If the forwarder has a display name set, it is shown instead of the ID.",
      },
      subscribed: {
        label: "Subscribed",
        summary: "Whether the receiver is actively receiving data from this stream.",
        detailHtml: "A subscribed stream actively delivers chip reads to the receiver's local port. Unsubscribing stops local delivery but does <strong>not</strong> stop the forwarder from sending data to the server. Data continues to accumulate on the server and can be replayed later.",
      },
      local_port: {
        label: "Local Port",
        summary: "The local port where reads from this stream are forwarded.",
        detailHtml: "The port on this machine where the receiver forwards chip reads from this stream. Your timing software should be configured to listen on this port.<br><br>The default port is calculated as <strong>10000 + the last octet of the reader's IP address</strong>. Custom ports can be set via <strong>Admin &gt; Port Overrides</strong>.",
      },
      stream_epoch: {
        label: "Epoch",
        summary: "The current epoch (data segment) the stream is reading from.",
        detailHtml: "An epoch represents a segment of timing data, typically corresponding to a race or wave. Epochs are numbered sequentially. The epoch name (if set) provides a human-readable label like 'Race 1' or 'Wave A'.",
      },
      earliest_epoch: {
        label: "Earliest Epoch Override",
        summary: "Skip historical epochs and start receiving from a specific epoch.",
        detailHtml: "In Live mode, you can set an earliest-epoch override to skip older data and only receive reads from a specific epoch onward. This is useful when you only care about the current race. Clear the override to receive all available data.",
      },
    },
    tips: [
      "Unsubscribing a stream only stops local delivery. Data continues to accumulate on the server and can be replayed later.",
      "If your timing software isn't receiving reads, check that it's listening on the correct local port.",
      "The 'degraded' indicator means the server reported an issue with this stream. Reads may still flow but check with the server operator.",
      "Use <strong>Admin &gt; Port Overrides</strong> to customize which local port each stream uses if the defaults don't match your timing software setup.",
    ],
    seeAlso: [{ sectionKey: "receiver_mode", label: "Receiver Mode" }],
  },
} as const satisfies HelpContext;
