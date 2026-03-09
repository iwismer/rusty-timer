import type { HelpContext } from "./help-types";

export const FORWARDER_HELP = {
  general: {
    title: "General Settings",
    overview: "Core forwarder identity settings.",
    fields: {
      display_name: {
        label: "Display Name",
        summary: "Optional friendly name to identify this forwarder in the UI.",
        detailHtml: "An optional human-readable name for this forwarder. When set, it appears instead of the forwarder ID in the server dashboard and receiver stream list. Useful when managing multiple forwarders at a race venue.",
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
        detailHtml: "The base URL of your remote timing server. Enter the HTTP or HTTPS URL (e.g. https://server.example.com:8080). The forwarder automatically converts this to a WebSocket connection for real-time data streaming. HTTPS URLs use secure WSS connections.",
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
    overview: "IPICO reader devices this forwarder connects to. Each entry represents a physical IPICO reader.",
    fields: {
      reader_ip: {
        label: "IP Address",
        summary: "Network address of the IPICO reader device.",
        detailHtml: "The IP address of the IPICO reader. Reader IP addresses are configured in the forwarder config file and shown here for reference. For a range of readers, the start IP and end octet define multiple readers on the same subnet.",
        recommended: "Use static IPs for readers to avoid DHCP reassignment during a race.",
      },
      reader_port: {
        label: "Reader Port",
        summary: "TCP port the reader listens on for connections.",
        detailHtml: "The TCP port used to connect to the IPICO reader. Most IPICO Lite readers use port 10000 by default, while Elite readers use port 10100.",
        default: "10000",
        range: "1-65535",
      },
      enabled: {
        label: "Enabled",
        summary: "Whether this reader is active and should be connected.",
        detailHtml: "Toggle reader on or off. Disabled readers are not connected. Use this to temporarily deactivate a reader without removing it from the configuration.",
        default: "Enabled",
      },
      default_local_port: {
        label: "Default Local Port",
        summary: "Auto-calculated local port based on reader IP (10000 + last octet).",
        detailHtml: "The default local forwarding port, calculated as 10000 + the last octet of the reader's IP address. For example, a reader at 192.168.0.50 gets local port 10050. The forwarder listens on this port and re-broadcasts reads from the reader, so any timing software on the local network can connect to this port to receive reads directly — independent of the upstream server connection.",
      },
      local_port_override: {
        label: "Local Port Override",
        summary: "Optional custom local port, overriding the auto-calculated default.",
        detailHtml: "Set a custom local forwarding port instead of the auto-calculated default. Use this when your timing software expects data on a specific port, or when multiple readers would otherwise share the same default port.",
        range: "1-65535",
      },
    },
    tips: [
      "At least one reader is required.",
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
        detailHtml: "The read mode controls how the IPICO reader processes chip reads before sending them to the forwarder.<br><br><strong>Raw</strong>: Every individual chip detection is sent as-is. This produces the highest volume of data and includes extraneous reads such as repeated detections of the same chip. Mainly useful for debugging or when you need access to every single antenna hit.<br><br><strong>Event</strong>: The reader sends one read per chip detection, then resends it after a timeout period as a retry mechanism. Uses the reader's internal deduplication logic with a fixed window set by the firmware.<br><br><strong>First/Last Seen (FS/LS)</strong>: The reader reports the first and last detection of each chip within a configurable timeout window. This gives you the timestamp of when a chip first entered range and when it last left range, which is ideal for calculating split times and finish times. The timeout window controls how long the reader waits after the last detection before finalizing the read.",
        default: "Raw",
        range: "Raw, Event, First/Last Seen",
        recommended: "First/Last Seen with a 5-second timeout for most race timing scenarios. FS/LS provides clean, deduplicated data with both entry and exit timestamps.",
      },
      timeout: {
        label: "Timeout",
        summary: "Seconds the reader waits after last detection before finalizing a read (FS/LS mode only).",
        detailHtml: "The deduplication timeout window in seconds, used only in First/Last Seen mode. After the reader detects a chip, it waits this many seconds for additional detections. If no new detections arrive within the timeout, the read is finalized and sent.<br><br>A shorter timeout means faster reporting but risks splitting a single pass into multiple reads. A longer timeout ensures complete pass detection but adds latency.<br><br>For most race timing, 5 seconds provides a good balance: fast enough for timely results, long enough to capture a complete pass through the timing mat.",
        default: "5",
        range: "1-255 seconds",
        recommended: "5 seconds for standard race timing. Use shorter (2-3s) for fast-paced events like cycling sprints. Use longer (8-10s) for crowded starts.",
      },
    },
    tips: [
      "Generally use First/Last Seen mode with a 5-second timeout for race timing. Raw mode generates extraneous data (repeated detections of the same chip) and Event mode's deduplication window is not configurable.",
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
        detailHtml: "When enabled, the 'Restart Forwarder Device' and 'Shutdown Forwarder Device' buttons in the Dangerous Actions section become available. This is a safety guard to prevent accidental power actions on the physical hardware. Restart Forwarder Service is always available regardless of this setting.",
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
        detailHtml: "Stops and restarts the forwarder service. The forwarder will disconnect from all readers and the server, then reconnect. Any unsent reads in the journal will be sent after reconnection. No data is lost thanks to the journal system.",
      },
      restart_device: {
        label: "Restart Forwarder Device",
        summary: "Reboots the physical forwarder hardware. Requires power actions enabled.",
        detailHtml: "Initiates a full reboot of the forwarder device (e.g. Raspberry Pi). The device will be offline for 30-60 seconds during restart. Requires 'Allow restart/shutdown' to be enabled in Forwarder Controls. All reads in the journal are preserved across reboots.",
      },
      shutdown_device: {
        label: "Shutdown Forwarder Device",
        summary: "Powers off the physical forwarder hardware. Requires power actions enabled.",
        detailHtml: "Initiates a clean shutdown of the forwarder device. The device will need to be physically powered back on. Only use this at the end of a race day or if you need to relocate the hardware. Requires 'Allow restart/shutdown' to be enabled in Forwarder Controls.",
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
        detailHtml: "The URL path appended to the server base URL for the WebSocket connection. In most setups this is auto-detected and does not need to be changed. Only modify this if your server is behind a reverse proxy that routes WebSocket connections to a non-standard path.",
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
        detailHtml: "The filesystem path to a file containing the raw authentication token. The forwarder reads this file on startup and sends it as a Bearer credential during the WebSocket handshake. Use WSS (WebSocket over TLS) to protect the token in transit. The server verifies the token by comparing its SHA-256 hash against stored credentials. Store the token file with restricted permissions (readable only by the forwarder service user).",
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
        detailHtml: "Path to the SQLite database file used for the forwarder's durable journal. When a server connection drops, reads accumulate in the journal and are sent when the connection is restored. An in-memory journal (empty path) is faster but loses data if the forwarder process or device restarts. A file-based journal persists reads across restarts.",
        default: "In-memory (empty path)",
        recommended: "Always use a file path for race day (e.g. /var/lib/rusty-timer/journal.db). In-memory is only acceptable for testing.",
      },
      prune_watermark_pct: {
        label: "Prune Watermark %",
        summary: "Triggers journal cleanup when the database reaches this fullness percentage.",
        detailHtml: "The journal prunes successfully-sent reads when the database size reaches this percentage of its capacity limit. Lower values prune more aggressively (freeing space sooner), higher values allow the journal to grow larger before cleaning up. Only acknowledged reads are pruned; unacknowledged reads are always preserved.",
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
        detailHtml: "Controls whether reads are sent to the server one at a time (immediate) or collected into batches (batched). Immediate mode has lower latency for individual reads. Batched mode is more efficient for high-volume scenarios with many readers, reducing network overhead.",
        default: "Immediate",
        range: "Immediate, Batched",
        recommended: "Immediate for most race setups. Use Batched only if you have many readers producing very high read volumes.",
      },
      batch_flush_ms: {
        label: "Batch Flush (ms)",
        summary: "Maximum time in milliseconds to wait before sending a batch.",
        detailHtml: "In batched mode, this is the maximum time the forwarder waits before sending a batch, even if the batch isn't full. Lower values reduce latency; higher values allow larger, more efficient batches. Only used when Batch Mode is set to Batched.",
        default: "100ms",
        range: "0+ milliseconds",
        recommended: "100ms provides a good balance. Increase to 500ms if bandwidth is very limited.",
      },
      batch_max_events: {
        label: "Batch Max Events",
        summary: "Maximum number of reads per batch before it's sent immediately.",
        detailHtml: "In batched mode, a batch is sent as soon as it reaches this many events, regardless of the flush timer. This prevents batches from growing too large. Only used when Batch Mode is set to Batched.",
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
        detailHtml: "The network address and port for the forwarder's built-in status HTTP server. This endpoint provides health checks and status information. Bind to 0.0.0.0 to allow access from any network interface, or 127.0.0.1 to restrict to local access only.",
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
        detailHtml: "Controls the forwarder's update behavior.\n\n<strong>Automatic</strong>: The forwarder checks for updates and downloads/applies them automatically. The service will restart to apply updates.\n\n<strong>Check Only</strong>: The forwarder checks for updates and notifies (via status) but does not download or apply them. Use this when you want to review updates before applying.\n\n<strong>Disabled</strong>: No update checking at all. Use this on race day to prevent any unexpected service restarts.",
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
  status_overview: {
    title: "Status",
    overview: "Live identity and health information for this forwarder. These fields are read-only and reflect the current state of the running service.",
    fields: {
      forwarder_id: {
        label: "Forwarder ID",
        summary: "Stable identifier for this forwarder, derived from its authentication token.",
        detailHtml: "A unique identifier automatically derived from the forwarder's authentication token (e.g. <code>fwd-3a9f1c2b8e4d07f1</code>). The ID is stable across restarts and reboots as long as the token file does not change. The server uses this ID to identify the forwarder in the dashboard and in the receiver stream list.",
      },
      version: {
        label: "Version",
        summary: "Software version currently running on this forwarder.",
        detailHtml: "The version of the forwarder service currently running. Use this to confirm that an update has been applied after a service restart. See the <strong>Update</strong> section for details on managing software updates.",
      },
      readiness: {
        label: "Readiness",
        summary: "Whether the forwarder's local subsystems have finished starting up and are operating normally.",
        detailHtml: "Readiness reflects whether the forwarder's internal subsystems — config loading, journal initialization, and worker task startup — have all completed successfully. It does <strong>not</strong> indicate whether the uplink to the server is connected; the forwarder is considered ready as soon as its local setup is complete.<br><br><strong>Ready</strong>: All subsystems initialized. The forwarder is collecting reads from configured readers and forwarding them when the server connection is available.<br><br><strong>Not ready</strong>: The forwarder is still starting up or a subsystem failed to initialize. The reason is shown in parentheses next to the badge (e.g. <em>starting</em>). This state is normal for a few seconds after the service starts. If it persists, check the log for initialization errors.",
      },
    },
    tips: [
      "The Forwarder ID is tied to the authentication token. If you rotate the token, the ID will change and the server will treat this as a new forwarder.",
      "'Not ready' is expected for a few seconds after the service starts or restarts. If it persists, check the Logs section for errors.",
      "Readiness does not depend on the server connection. A forwarder can be ready and collecting reads even while the uplink is disconnected — reads accumulate in the journal and are sent when the connection is restored.",
    ],
    seeAlso: [
      { sectionKey: "auth", label: "Authentication" },
      { sectionKey: "journal", label: "Journal" },
      { sectionKey: "server", label: "Server Connection" },
    ],
  },
  service_overview: {
    title: "Service",
    overview: "Live status of the forwarder service and its connection to the remote server.",
    fields: {
      uplink: {
        label: "Uplink",
        summary: "Whether the forwarder is currently connected to the remote server.",
        detailHtml: "Shows the state of the forwarder's WebSocket connection to the remote server.<br><br><strong>Connected</strong>: The forwarder has completed the handshake and is actively sending reads.<br><br><strong>Disconnected</strong>: The forwarder is not currently connected — reads continue to accumulate in the journal and will be sent automatically when the connection is restored.<br><br>Uplink state does not affect the forwarder's readiness to collect reads from readers.",
      },
      restart_needed: {
        label: "Restart Needed",
        summary: "Whether a saved configuration change is waiting to take effect.",
        detailHtml: "Shows <strong>Pending</strong> when a configuration change has been saved but not yet applied. Configuration changes are written to the config file immediately, but the running forwarder process must restart to read them. Click <strong>Restart Now</strong> to apply the changes. Shows <strong>None</strong> when the running process reflects the current configuration.",
      },
    },
    tips: [
      "A disconnected uplink does not lose reads. The journal stores all reads and replays them to the server once the connection recovers.",
      "If the uplink stays disconnected, verify the server URL and authentication token are correct, and that the server is reachable from the forwarder's network.",
      "Restart Now restarts the forwarder service, not the physical device. Readers will briefly disconnect and reconnect. No reads are lost thanks to the journal.",
      "On race day, apply any config changes and restart before the first race so the forwarder is stable during timing.",
    ],
    seeAlso: [
      { sectionKey: "server", label: "Server Connection" },
      { sectionKey: "journal", label: "Journal" },
      { sectionKey: "dangerous_actions", label: "Dangerous Actions" },
    ],
  },
  reader_live: {
    title: "Reader Live Status",
    overview: "Real-time statistics and controls for an active reader connection. These values update automatically while the page is open.",
    fields: {
      reads_session: {
        label: "Reads (Session)",
        summary: "Number of chip reads received from this reader since the forwarder service last started.",
        detailHtml: "A running count of chip reads received from this reader since the forwarder service was last started (or restarted). This counter resets to zero each time the forwarder service restarts — it reflects the current service session only, not historical data. Use this to confirm reads are actively flowing from a reader during the current session.",
      },
      reads_total: {
        label: "Reads (Total)",
        summary: "Total chip reads from this reader recorded in the journal, across all sessions.",
        detailHtml: "The cumulative count of chip reads from this reader stored in the forwarder's journal database. At startup, this value is loaded from the journal (all epochs, all sessions) and then incremented in memory as new reads arrive. This count persists across service restarts as long as a file-based journal is configured. If using an in-memory journal, this counter resets on every service restart and will match the session count. Use this to get a sense of total throughput from a reader over the course of an event.",
      },
      local_port: {
        label: "Local Port",
        summary: "Port on this device where the reader's chip reads are available for local timing software.",
        detailHtml: "The forwarder opens a TCP listener on this port and re-broadcasts every chip read it receives from this reader. Timing software running on the same network can connect here to receive reads directly, independently of the upstream server connection. The port is either auto-calculated as 10000 + the last octet of the reader's IP address (e.g. a reader at 192.168.0.50 uses port 10050), or a fixed value when a Local Port Override is configured for this reader.",
      },
      last_seen: {
        label: "Last Seen",
        summary: "How long ago the most recent chip read was received from this reader.",
        detailHtml: "The time elapsed since the forwarder last received a chip read from this reader. Updates automatically while the page is open — the displayed value ticks forward in real time so you can see how stale the data is without refreshing. Shows <strong>never</strong> if no reads have been received in the current session. A rapidly increasing value while the reader is connected may indicate the timing mat is idle or no chips are in range.",
      },
      epoch_name: {
        label: "Epoch Name",
        summary: "Optional label for the current epoch on this reader, e.g. 'Race 1' or 'Wave 2'.",
        detailHtml: "Assigns a human-readable name to the reader's current epoch. The name is saved to the server and displayed above the input as the active epoch label. Clearing the field and saving removes the name. The name applies to the current epoch only — after advancing to a new epoch, set a new name to identify it.",
      },
      advance_epoch: {
        label: "Advance Epoch",
        summary: "Starts a new epoch for this reader, separating subsequent reads from previous ones.",
        detailHtml: "Increments the reader's stream epoch counter by one and resets its sequence number to 1. All reads from this point forward are recorded under the new epoch, allowing the server and receiver to distinguish them from reads in the previous epoch. Reads already captured in earlier epochs are not deleted and will still be delivered if unacknowledged. Use this at the start of each race or wave to create a clean separation in the read stream. After advancing, set an epoch name to identify the new segment.",
      },
      clock_drift: {
        label: "Clock Drift",
        summary: "Difference between the reader's internal clock and the forwarder's local clock, in milliseconds.",
        detailHtml:
          "Shows how far the reader's clock deviates from the forwarder's system clock at the time of the last clock read. " +
          "The value is computed as <em>forwarder time \u2212 reader time</em>: a positive value means the reader clock is running behind, " +
          "a negative value means it is running ahead. The forwarder reads the reader clock automatically on connect and periodically " +
          "during the session, so the value updates without manual intervention." +
          "<br><br>" +
          "The color indicates severity:" +
          "<ul>" +
          '<li><strong class="text-green-500">Green</strong> \u2014 less than 100\u2009ms. Normal operating range; no action needed.</li>' +
          '<li><strong class="text-yellow-500">Yellow</strong> \u2014 100\u2009ms to 499\u2009ms. Noticeable drift; consider syncing before the next race.</li>' +
          '<li><strong class="text-red-500">Red</strong> \u2014 500\u2009ms or more. Significant drift that will affect timestamp accuracy; sync the clock now.</li>' +
          "</ul>" +
          "A dash (\u2014) means the reader clock has not been read yet in the current session." +
          "<br><br>" +
          "Use <strong>Sync Clock</strong> to correct the drift. The sync procedure probes round-trip network latency and schedules " +
          "the set command to land precisely on a whole-second boundary, reducing residual drift to approximately 25\u2009ms.",
      },
      tto_bytes: {
        label: "TTO Bytes",
        summary: "Adds extra timestamp data to each chip read: antenna index, page, and first/last-seen flags.",
        detailHtml:
          "TTO (Time To Own) is an IPICO reader feature that appends 3 extra bytes to every chip read message. When enabled, each read includes an antenna <strong>index</strong>, a <strong>page</strong> number, and a flags byte that encodes whether the detection was the <strong>first seen</strong> or <strong>last seen</strong> event within a First/Last Seen pass, plus a <strong>tamper</strong> flag.\n\n" +
          "The setting is written directly to the reader\u2019s tag message format register using the IPICO control protocol \u2014 it takes effect immediately and persists in the reader\u2019s own memory across power cycles. Toggling TTO does not require restarting the forwarder service.\n\n" +
          "<strong>Enabled</strong>: Each chip read message is 42 characters instead of the standard 36. The extra bytes provide per-read antenna and pass-direction metadata.\n\n" +
          "<strong>Disabled</strong>: Standard 36-character reads with no extra bytes. Compatible with all timing software that uses the IPICO format.\n\n" +
          "TTO is not required for normal race timing. Enable it only if your timing software or post-processing workflow specifically uses the antenna index, page, or first/last-seen flags.",
        default: "Disabled",
        recommended: "Leave disabled unless your timing software explicitly uses TTO metadata.",
      },
      sync_clock: {
        label: "Sync Clock",
        summary: "Synchronizes the reader's internal clock to the forwarder's local time.",
        detailHtml: "Sends the current time from the forwarder to the reader using a latency-compensated algorithm. The forwarder first probes the round-trip time to the reader, then times the SET_DATE_TIME command so that the reader's new second takes effect precisely on a whole-second boundary. After the sync completes, the forwarder reads the clock back and reports the residual drift in milliseconds.\n\nAccurate chip-read timestamps depend on the reader's clock. Sync the clock before each race \u2014 especially after the reader has been powered on for the first time, after a long idle period, or if you notice timestamp anomalies in timing results. The button is only available while the reader is connected.",
        recommended: "Sync the clock at the start of each race day and again before each race if high timestamp accuracy is required.",
      },
      refresh_reader: {
        label: "Refresh",
        summary: "Re-polls the reader for its current status, firmware, configuration, and clock.",
        detailHtml: "Queries the reader over the control connection and updates all displayed reader info fields: extended status (recording state, estimated stored reads), read mode configuration, TTO reporting state, and clock. The forwarder polls this information automatically on connect, but you can use Refresh at any time to get the latest values without waiting for the next automatic poll \u2014 for example, after changing settings directly on the reader, or to confirm a previous command took effect.",
      },
      recording: {
        label: "Start / Stop Recording",
        summary: "Toggles whether the reader is internally recording chip reads to its onboard storage.",
        detailHtml: "Controls the reader's onboard recording state. When <strong>recording is on</strong>, the reader stores each chip read in its internal EEPROM memory in addition to streaming reads live to the forwarder. When <strong>recording is off</strong>, the reader streams reads but does not write them to onboard storage.\n\nOnboard recording is independent of the live data stream: reads are forwarded to the server regardless of recording state. Use recording as a safety net \u2014 if the forwarder loses its connection mid-race, the reads are preserved on the reader and can be retrieved later with <strong>Download Reads</strong>.\n\nThe button label and color reflect the current state: green <em>Start Recording</em> when recording is off, red <em>Stop Recording</em> when recording is on.",
        recommended: "Turn recording on before each race as a safety net. Download and clear records after each event to keep the reader's storage free for the next race.",
      },
      download_reads: {
        label: "Download Reads",
        summary: "Downloads all chip reads stored in the reader's onboard memory to the forwarder.",
        detailHtml: "Initiates a transfer of all records currently stored in the reader's onboard EEPROM to the forwarder. The download runs as a background task \u2014 a progress bar appears below the buttons showing reads received and estimated completion percentage. The forwarder processes incoming reads and routes them through the normal journal and uplink pipeline, so downloaded reads are delivered to the server just like live reads.\n\nDownload is the primary recovery path after a connection outage: if the forwarder lost its uplink during a race and reads were captured to onboard storage, use Download Reads once connectivity is restored to retrieve them.\n\nOnly one download can run at a time per reader. After a successful download, use <strong>Clear Records</strong> to free the reader's storage for the next race.",
      },
      clear_records: {
        label: "Clear Records",
        summary: "Erases all stored records from the reader's onboard EEPROM memory.",
        detailHtml: "Permanently erases all chip reads stored in the reader's internal memory. This is a multi-step hardware operation that takes approximately 10 seconds to complete.\n\n<strong>This action is irreversible.</strong> Always use <strong>Download Reads</strong> first if you need to recover the stored data before clearing. Clear records after each event to ensure the reader's storage is empty and ready for the next race. A full reader may not be able to store new reads.",
        recommended: "Always download reads before clearing. Clear records at the end of each race day so stored reads are available as a backup if the live stream had any gaps.",
      },
    },
    tips: [
      "Use 'Advance Epoch' at the start of each race or wave to cleanly separate reads in the data stream.",
      "If Reads (session) stops increasing while the reader is connected, check that chips are in range of the timing mat and that the reader is in the correct read mode.",
      "Last Seen shows 'never' until the first chip read arrives in the current session. This is normal before a race starts.",
      "Sync the clock before each race. Always download reads before clearing records.",
    ],
    seeAlso: [
      { sectionKey: "readers", label: "Reader Devices" },
      { sectionKey: "read_mode", label: "Read Mode" },
      { sectionKey: "journal", label: "Journal" },
    ],
  },
} as const satisfies HelpContext;
