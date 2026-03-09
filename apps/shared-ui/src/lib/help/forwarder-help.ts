import type { HelpContext } from "./help-types";

export const FORWARDER_HELP: HelpContext = {
  general: {
    title: "General Settings",
    overview: "Core forwarder identity settings.",
    fields: {
      display_name: {
        label: "Display Name",
        summary: "Optional friendly name to identify this forwarder in the UI.",
        detail: "An optional human-readable name for this forwarder. When set, it appears instead of the forwarder ID in the server dashboard and receiver stream list. Useful when managing multiple forwarders at a race venue.",
        default: "None (optional)",
      },
    },
    tips: [
      "Give each forwarder a descriptive name like 'Start Line' or 'Finish Line A' to make it easy to identify on race day.",
    ],
    seeAlso: [{ sectionKey: "server", label: "Server Connection" }],
  },
  server: {
    title: "Server Connection",
    overview: "Configure how the forwarder connects to the remote server to send timing data.",
    fields: {
      base_url: {
        label: "Base URL",
        summary: "The HTTP(S) URL of the server this forwarder sends data to.",
        detail: "The base URL of your remote timing server. Enter the HTTP or HTTPS URL (e.g. https://server.example.com:8080). The forwarder automatically converts this to a WebSocket connection for real-time data streaming. HTTPS URLs use secure WSS connections.",
        default: "None (required)",
        recommended: "Use HTTPS for production to encrypt timing data in transit.",
      },
    },
    tips: [
      "If the forwarder can't connect, verify the server URL is reachable from the forwarder's network. Check for firewall rules blocking the port.",
      "The forwarder will automatically reconnect if the connection drops. Check the log for connection status.",
    ],
    seeAlso: [
      { sectionKey: "ws_path", label: "WebSocket Path" },
      { sectionKey: "auth", label: "Authentication" },
    ],
  },
  readers: {
    title: "Reader Devices",
    overview: "IPICO reader devices this forwarder connects to. Each reader represents a physical timing mat or antenna.",
    fields: {
      reader_ip: {
        label: "IP Address",
        summary: "Network address of the IPICO reader device.",
        detail: "The IP address of the IPICO reader. For a single reader, enter the full IP (e.g. 192.168.0.50). For a range of readers, enter the start IP and end octet to connect to multiple readers on the same subnet.",
        recommended: "Use static IPs for readers to avoid DHCP reassignment during a race.",
      },
      reader_port: {
        label: "Reader Port",
        summary: "TCP port the reader listens on for connections.",
        detail: "The TCP port used to connect to the IPICO reader. Most IPICO readers use port 10000 by default.",
        default: "10000",
        range: "1-65535",
      },
      enabled: {
        label: "Enabled",
        summary: "Whether this reader is active and should be connected.",
        detail: "Toggle reader on or off. Disabled readers are not connected. Use this to temporarily deactivate a reader without removing it from the configuration.",
        default: "Enabled",
      },
      default_local_port: {
        label: "Default Local Port",
        summary: "Auto-calculated local port based on reader IP (10000 + last octet).",
        detail: "The default local forwarding port, calculated as 10000 + the last octet of the reader's IP address. For example, a reader at 192.168.0.50 gets local port 10050. This port is used by the receiver to forward reads to timing software.",
      },
      local_port_override: {
        label: "Local Port Override",
        summary: "Optional custom local port, overriding the auto-calculated default.",
        detail: "Set a custom local forwarding port instead of the auto-calculated default. Use this when your timing software expects data on a specific port, or when multiple readers would otherwise share the same default port.",
        range: "1-65535",
      },
    },
    tips: [
      "At least one reader is required. Add all readers before race day and verify connectivity.",
      "Use IP ranges when you have consecutive readers on the same subnet (e.g. 192.168.0.150 through 192.168.0.160).",
      "If reads aren't appearing, check that the reader IP is correct and the reader is powered on and connected to the network.",
    ],
    seeAlso: [{ sectionKey: "read_mode", label: "Read Mode" }],
  },
  read_mode: {
    title: "Read Mode",
    overview: "Controls how the IPICO reader processes and reports chip reads. The read mode determines deduplication behavior and timing resolution.",
    fields: {
      read_mode: {
        label: "Read Mode",
        summary: "How the reader reports chip reads: Raw, Event, or First/Last Seen.",
        detail: "The read mode controls how the IPICO reader processes chip reads before sending them to the forwarder.\n\n<strong>Raw</strong>: Every individual chip detection is sent as-is. This produces the highest volume of data and is mainly useful for debugging or when you need access to every single antenna hit. Not recommended for race timing due to high volume.\n\n<strong>Event</strong>: The reader buffers reads and reports one event per chip per pass. Uses the reader's internal deduplication logic. Produces less data than Raw but the deduplication window is fixed by the reader firmware.\n\n<strong>First/Last Seen (FS/LS)</strong>: The reader reports the first and last detection of each chip within a configurable timeout window. This gives you the timestamp of when a chip first entered range and when it last left range, which is ideal for calculating split times and finish times. The timeout window controls how long the reader waits after the last detection before finalizing the read.",
        default: "Raw",
        range: "Raw, Event, First/Last Seen",
        recommended: "First/Last Seen with a 5-second timeout for most race timing scenarios. FS/LS provides clean, deduplicated data with both entry and exit timestamps.",
      },
      timeout: {
        label: "Timeout",
        summary: "Seconds the reader waits after last detection before finalizing a read (FS/LS mode only).",
        detail: "The deduplication timeout window in seconds, used only in First/Last Seen mode. After the reader detects a chip, it waits this many seconds for additional detections. If no new detections arrive within the timeout, the read is finalized and sent. A shorter timeout means faster reporting but risks splitting a single pass into multiple reads. A longer timeout ensures complete pass detection but adds latency.\n\nFor most race timing, 5 seconds provides a good balance: fast enough for timely results, long enough to capture a complete pass through the timing mat.",
        default: "5",
        range: "1-255 seconds",
        recommended: "5 seconds for standard race timing. Use shorter (2-3s) for fast-paced events like cycling sprints. Use longer (8-10s) for crowded starts.",
      },
    },
    tips: [
      "Always use First/Last Seen mode with a 5-second timeout for race timing. Raw mode generates too much data and Event mode's deduplication window is not configurable.",
      "If you're seeing duplicate reads in your timing software, increase the timeout to ensure complete pass detection.",
      "Changing the read mode takes effect immediately. The reader will briefly pause reads during the mode switch.",
    ],
    seeAlso: [{ sectionKey: "readers", label: "Reader Devices" }],
  },
  controls: {
    title: "Forwarder Controls",
    overview: "Service-level and device-level power management options for the forwarder hardware.",
    fields: {
      allow_power_actions: {
        label: "Allow Restart/Shutdown",
        summary: "Enables the restart and shutdown buttons for the physical forwarder device.",
        detail: "When enabled, the 'Restart Forwarder Device' and 'Shutdown Forwarder Device' buttons in the Dangerous Actions section become available. This is a safety guard to prevent accidental power actions on the physical hardware. Restart Forwarder Service is always available regardless of this setting.",
        default: "Disabled",
        recommended: "Keep disabled during active timing to prevent accidental shutdowns.",
      },
    },
    tips: [
      "Enable power actions only when you need to restart or shut down the physical device. Disable again after use.",
      "Restarting the forwarder service (not device) preserves network connections and is usually sufficient for troubleshooting.",
    ],
    seeAlso: [{ sectionKey: "dangerous_actions", label: "Dangerous Actions" }],
  },
  dangerous_actions: {
    title: "Dangerous Actions",
    overview: "Actions that affect forwarder availability. Use with caution during active timing.",
    fields: {
      restart_service: {
        label: "Restart Forwarder Service",
        summary: "Restarts the forwarder software process without rebooting the device.",
        detail: "Stops and restarts the forwarder service. The forwarder will disconnect from all readers and the server, then reconnect. Any unsent reads in the journal will be sent after reconnection. No data is lost thanks to the journal system.",
      },
      restart_device: {
        label: "Restart Forwarder Device",
        summary: "Reboots the physical forwarder hardware. Requires power actions enabled.",
        detail: "Initiates a full reboot of the forwarder device (e.g. Raspberry Pi). The device will be offline for 30-60 seconds during restart. Requires 'Allow restart/shutdown' to be enabled in Forwarder Controls. All reads in the journal are preserved across reboots.",
      },
      shutdown_device: {
        label: "Shutdown Forwarder Device",
        summary: "Powers off the physical forwarder hardware. Requires power actions enabled.",
        detail: "Initiates a clean shutdown of the forwarder device. The device will need to be physically powered back on. Only use this at the end of a race day or if you need to relocate the hardware. Requires 'Allow restart/shutdown' to be enabled in Forwarder Controls.",
      },
    },
    tips: [
      "On race day, prefer 'Restart Service' over 'Restart Device' unless the device is unresponsive.",
      "After a device restart, verify all readers reconnect and reads are flowing before the next race starts.",
      "Shutdown should only be used at the end of the event. The device must be physically powered back on.",
    ],
    seeAlso: [{ sectionKey: "controls", label: "Forwarder Controls" }],
  },
  ws_path: {
    title: "WebSocket Path",
    overview: "Advanced setting for the WebSocket endpoint path used to connect to the server.",
    fields: {
      forwarders_ws_path: {
        label: "WebSocket Path",
        summary: "Custom WebSocket endpoint path on the server. Usually auto-detected.",
        detail: "The URL path appended to the server base URL for the WebSocket connection. In most setups this is auto-detected and does not need to be changed. Only modify this if your server is behind a reverse proxy that routes WebSocket connections to a non-standard path.",
        default: "Auto-detected",
        recommended: "Leave empty unless your server admin has provided a custom path.",
      },
    },
    tips: [
      "Only change this if instructed by your server administrator. An incorrect path will prevent the forwarder from connecting.",
    ],
    seeAlso: [{ sectionKey: "server", label: "Server Connection" }],
  },
  auth: {
    title: "Authentication",
    overview: "Configure the authentication token used to connect to the server.",
    fields: {
      token_file: {
        label: "Token File Path",
        summary: "Path to a file containing the authentication token.",
        detail: "The filesystem path to a file containing the raw authentication token. The forwarder reads this file on startup and uses the token to authenticate with the server. The token is hashed (SHA-256) before transmission. Store the token file with restricted permissions (readable only by the forwarder service user).",
        default: "None (required for authenticated servers)",
        recommended: "Use a dedicated token file with restricted permissions. Do not embed tokens in the config file.",
      },
    },
    tips: [
      "If authentication fails, verify the token file exists at the specified path and contains the correct token.",
      "The token must match what the server expects. Generate tokens using the server's token management tools.",
    ],
    seeAlso: [{ sectionKey: "server", label: "Server Connection" }],
  },
  journal: {
    title: "Journal",
    overview: "The journal provides durable storage for chip reads, ensuring no data is lost if the server connection drops.",
    fields: {
      sqlite_path: {
        label: "SQLite Path",
        summary: "File path for the SQLite journal database. Leave empty for in-memory.",
        detail: "Path to the SQLite database file used for the forwarder's durable journal. When a server connection drops, reads accumulate in the journal and are sent when the connection is restored. An in-memory journal (empty path) is faster but loses data if the forwarder process or device restarts. A file-based journal persists reads across restarts.",
        default: "In-memory (empty path)",
        recommended: "Always use a file path for race day (e.g. /var/lib/rusty-timer/journal.db). In-memory is only acceptable for testing.",
      },
      prune_watermark_pct: {
        label: "Prune Watermark %",
        summary: "Triggers journal cleanup when the database reaches this fullness percentage.",
        detail: "The journal prunes successfully-sent reads when the database size reaches this percentage of its capacity limit. Lower values prune more aggressively (freeing space sooner), higher values allow the journal to grow larger before cleaning up. Only acknowledged reads are pruned; unacknowledged reads are always preserved.",
        default: "80%",
        range: "0-100%",
        recommended: "80% works well for most scenarios. Lower to 50% if running on storage-constrained devices.",
      },
    },
    tips: [
      "In-memory journal is fine for testing but ALWAYS use a file path for race day to prevent data loss on restart.",
      "If the journal file grows very large, it means reads are accumulating faster than they can be sent. Check the server connection.",
      "The journal provides at-least-once delivery: reads may be sent more than once but never lost.",
    ],
    seeAlso: [{ sectionKey: "uplink", label: "Uplink (Batching)" }],
  },
  uplink: {
    title: "Uplink (Batching)",
    overview: "Controls how reads are batched and sent from the forwarder to the server over the WebSocket connection.",
    fields: {
      batch_mode: {
        label: "Batch Mode",
        summary: "Send reads immediately one at a time, or batch multiple reads together.",
        detail: "Controls whether reads are sent to the server one at a time (immediate) or collected into batches (batched). Immediate mode has lower latency for individual reads. Batched mode is more efficient for high-volume scenarios with many readers, reducing network overhead.",
        default: "Immediate",
        range: "Immediate, Batched",
        recommended: "Immediate for most race setups. Use Batched only if you have many readers producing very high read volumes.",
      },
      batch_flush_ms: {
        label: "Batch Flush (ms)",
        summary: "Maximum time in milliseconds to wait before sending a batch.",
        detail: "In batched mode, this is the maximum time the forwarder waits before sending a batch, even if the batch isn't full. Lower values reduce latency; higher values allow larger, more efficient batches. Only used when Batch Mode is set to Batched.",
        default: "100ms",
        range: "0+ milliseconds",
        recommended: "100ms provides a good balance. Increase to 500ms if bandwidth is very limited.",
      },
      batch_max_events: {
        label: "Batch Max Events",
        summary: "Maximum number of reads per batch before it's sent immediately.",
        detail: "In batched mode, a batch is sent as soon as it reaches this many events, regardless of the flush timer. This prevents batches from growing too large. Only used when Batch Mode is set to Batched.",
        default: "1000",
        range: "1+",
        recommended: "1000 is sufficient for most scenarios. Reduce if you need lower latency.",
      },
    },
    tips: [
      "For most race timing, keep Immediate mode. Batching is an optimization for high-throughput scenarios.",
      "If reads appear delayed, check that batch_flush_ms isn't set too high.",
    ],
    seeAlso: [{ sectionKey: "journal", label: "Journal" }],
  },
  status_http: {
    title: "Status HTTP",
    overview: "The forwarder exposes a local HTTP endpoint for health monitoring and status checks.",
    fields: {
      bind: {
        label: "Bind Address",
        summary: "IP:port the status HTTP server listens on.",
        detail: "The network address and port for the forwarder's built-in status HTTP server. This endpoint provides health checks and status information. Bind to 0.0.0.0 to allow access from any network interface, or 127.0.0.1 to restrict to local access only.",
        default: "0.0.0.0:8080",
        range: "Valid IP:port combination",
        recommended: "0.0.0.0:8080 for standard setups. Use 127.0.0.1:8080 if you don't need remote status access.",
      },
    },
    tips: [
      "The status endpoint is useful for monitoring forwarder health from the server or a separate monitoring tool.",
      "If the status port conflicts with another service, change it to an unused port.",
    ],
  },
  update: {
    title: "Update",
    overview: "Controls how the forwarder checks for and applies software updates.",
    fields: {
      update_mode: {
        label: "Update Mode",
        summary: "How the forwarder handles software updates: automatic, check-only, or disabled.",
        detail: "Controls the forwarder's update behavior.\n\n<strong>Automatic</strong>: The forwarder checks for updates and downloads/applies them automatically. The service will restart to apply updates.\n\n<strong>Check Only</strong>: The forwarder checks for updates and notifies (via status) but does not download or apply them. Use this when you want to review updates before applying.\n\n<strong>Disabled</strong>: No update checking at all. Use this on race day to prevent any unexpected service restarts.",
        default: "Automatic",
        range: "Automatic, Check Only, Disabled",
        recommended: "Set to Disabled on race day to prevent unexpected restarts. Use Automatic or Check Only during setup.",
      },
    },
    tips: [
      "IMPORTANT: Set Update Mode to Disabled on race day to prevent unexpected service restarts during timing.",
      "Use 'Check Now' to manually trigger an update check regardless of the mode setting.",
      "After an update is applied, verify all readers reconnect and the forwarder is functioning correctly.",
    ],
    seeAlso: [{ sectionKey: "controls", label: "Forwarder Controls" }],
  },
};
