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
        summary: "The URL of the server this forwarder sends data to.",
        detailHtml: "The base URL of your remote timing server (e.g. <code>https://server.example.com:8080</code>). Use HTTPS to encrypt timing data in transit.",
        default: "None (required)",
        recommended: "Use HTTPS for production to encrypt timing data in transit.",
      },
    },
    tips: [
      "If the forwarder can't connect, verify the server URL is reachable from the forwarder's network. Check for firewall rules blocking the port.",
      "The forwarder will automatically reconnect if the connection drops. Check the log for connection status.",
    ],
    seeAlso: [
      { sectionKey: "ws_path", label: "Connection Path" },
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
        detailHtml: "The IP address of the IPICO reader. For a range of readers, the start IP and end octet define multiple readers on the same subnet.",
        recommended: "Use static IPs for readers to avoid DHCP reassignment during a race.",
      },
      reader_port: {
        label: "Reader Port",
        summary: "Port the reader listens on for connections.",
        detailHtml: "The port used to connect to the IPICO reader. Most IPICO Lite readers use port 10000 by default, while Elite readers use port 10100.",
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
        detailHtml: "The default local forwarding port, calculated as <strong>10000 + the last octet of the reader's IP address</strong>. For example, a reader at 192.168.0.50 gets local port 10050.<br><br>Timing software on the local network can connect to this port to receive reads directly.",
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
    overview: "Controls how the IPICO reader processes and reports chip reads.",
    fields: {
      read_mode: {
        label: "Read Mode",
        summary: "How the reader reports chip reads: Raw, Event, or First/Last Seen.",
        detailHtml: "The read mode controls how the IPICO reader processes chip reads before sending them to the forwarder." +
          "<ul>" +
          "<li><strong>Raw</strong>: Every individual chip detection is sent as-is. Produces the highest data volume and includes repeated detections. Mainly useful for debugging.</li>" +
          "<li><strong>Event</strong>: One read per chip detection, resent after a timeout as a retry. Uses the reader's built-in deduplication.</li>" +
          "<li><strong>First/Last Seen (FS/LS)</strong>: Reports the first and last detection of each chip within a configurable timeout window — ideal for split times and finish times.</li>" +
          "</ul>",
        default: "Raw",
        range: "Raw, Event, First/Last Seen",
        recommended: "First/Last Seen with a 5-second timeout for most race timing scenarios.",
      },
      timeout: {
        label: "Timeout",
        summary: "Seconds the reader waits after last detection before finalizing a read (FS/LS mode only).",
        detailHtml: "The deduplication timeout in seconds, used only in First/Last Seen mode. After detecting a chip, the reader waits this many seconds for additional detections. If no new detections arrive within the timeout, the read is finalized and sent.<br><br>A shorter timeout means faster reporting but risks splitting a single pass into multiple reads. A longer timeout ensures complete pass detection but adds latency.",
        default: "5",
        range: "1-255 seconds",
        recommended: "5 seconds for standard race timing. Use shorter (2-3s) for fast-paced events like cycling sprints. Use longer (8-10s) for crowded starts.",
      },
    },
    tips: [
      "Use First/Last Seen mode with a 5-second timeout for most race timing. Raw mode generates excessive data and Event mode's deduplication window is not configurable.",
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
        detailHtml: "When enabled, the 'Restart Forwarder Device' and 'Shutdown Forwarder Device' buttons become available. This is a safety guard to prevent accidental power actions on the physical hardware.<br><br>'Restart Forwarder Service' is always available regardless of this setting.",
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
        detailHtml: "Stops and restarts the forwarder service. The forwarder will disconnect from all readers and the server, then reconnect. No data is lost — any unsent reads are preserved and sent after reconnection.",
      },
      restart_device: {
        label: "Restart Forwarder Device",
        summary: "Reboots the physical forwarder hardware. Requires power actions enabled.",
        detailHtml: "Initiates a full reboot of the forwarder device. The device will be offline for 30-60 seconds during restart. Requires 'Allow Restart/Shutdown' to be enabled in Forwarder Controls. All reads are preserved across reboots.",
      },
      shutdown_device: {
        label: "Shutdown Forwarder Device",
        summary: "Powers off the physical forwarder hardware. Requires power actions enabled.",
        detailHtml: "Initiates a clean shutdown of the forwarder device. The device will need to be physically powered back on. Only use this at the end of a race day or if you need to relocate the hardware. Requires 'Allow Restart/Shutdown' to be enabled in Forwarder Controls.",
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
    title: "Connection Path",
    overview: "Advanced setting for the connection path used to connect to the server.",
    fields: {
      forwarders_ws_path: {
        label: "Connection Path",
        summary: "Custom connection path on the server. Usually auto-detected.",
        detailHtml: "The path appended to the server base URL for the connection. In most setups this is auto-detected and does not need to be changed. Only modify this if your server administrator has provided a custom path.",
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
        detailHtml: "The file path to the authentication token. The forwarder reads this file on startup and uses it to authenticate with the server. The token must match what the server expects.",
        default: "None (required for authenticated servers)",
        recommended: "Use a dedicated token file rather than embedding the token in the config file.",
      },
    },
    tips: [
      "If authentication fails, verify the token file exists at the specified path and contains the correct token.",
      "The token must match what the server expects. Contact the server operator if you need a new token.",
    ],
    seeAlso: [{ sectionKey: "server", label: "Server Connection" }],
  },
  journal: {
    title: "Journal",
    overview: "The journal provides durable storage for chip reads, ensuring no data is lost if the server connection drops.",
    fields: {
      sqlite_path: {
        label: "Journal File Path",
        summary: "File path for the journal storage. Leave empty for in-memory.",
        detailHtml: "Path to the file used for the forwarder's durable journal.<br><br>An in-memory journal (empty path) is faster but loses data if the forwarder restarts. A file-based journal persists reads across restarts.",
        default: "In-memory (empty path)",
        recommended: "Always use a file path for race day (e.g. /var/lib/rusty-timer/journal.db). In-memory is only acceptable for testing.",
      },
      prune_watermark_pct: {
        label: "Storage Cleanup Threshold",
        summary: "Triggers journal cleanup when storage reaches this fullness percentage.",
        detailHtml: "The journal cleans up already-sent reads when storage reaches this percentage of its capacity. Lower values free space sooner, higher values allow the journal to grow larger before cleaning up. Unsent reads are always preserved.",
        default: "80%",
        range: "0-100%",
        recommended: "80% works well for most scenarios. Lower to 50% if running on storage-constrained devices.",
      },
    },
    tips: [
      "In-memory journal is fine for testing but <strong>always</strong> use a file path for race day to prevent data loss on restart.",
      "If the journal file grows very large, it means reads are accumulating faster than they can be sent. Check the server connection.",
      "The journal provides at-least-once delivery: reads may be sent more than once but are never lost.",
    ],
    seeAlso: [{ sectionKey: "uplink", label: "Uplink (Batching)" }],
  },
  uplink: {
    title: "Uplink (Batching)",
    overview: "Controls how reads are batched and sent from the forwarder to the server.",
    fields: {
      batch_mode: {
        label: "Batch Mode",
        summary: "Send reads immediately one at a time, or batch multiple reads together.",
        detailHtml: "Controls whether reads are sent to the server one at a time (immediate) or collected into batches. Immediate mode has lower latency. Batched mode is more efficient when multiple readers are producing high volumes of reads.",
        default: "Immediate",
        range: "Immediate, Batched",
        recommended: "Immediate for most race setups. Use Batched only if you have many readers producing very high read volumes.",
      },
      batch_flush_ms: {
        label: "Send Delay (ms)",
        summary: "Maximum time in milliseconds to wait before sending a batch.",
        detailHtml: "In batched mode, this is the maximum time the forwarder waits before sending a batch, even if it isn't full. Lower values reduce latency; higher values allow larger, more efficient batches. Only applies when Batch Mode is set to Batched.",
        default: "100ms",
        range: "0+ milliseconds",
        recommended: "100ms provides a good balance. Increase to 500ms if bandwidth is very limited.",
      },
      batch_max_events: {
        label: "Max Reads per Batch",
        summary: "Maximum number of reads per batch before it's sent immediately.",
        detailHtml: "In batched mode, a batch is sent as soon as it reaches this many reads, regardless of the flush timer. Only applies when Batch Mode is set to Batched.",
        default: "1000",
        range: "1+",
        recommended: "1000 is sufficient for most scenarios. Reduce if you need lower latency.",
      },
    },
    tips: [
      "For most race timing, keep Immediate mode. Batching is an optimization for high-throughput scenarios.",
      "If reads appear delayed, check that Batch Flush isn't set too high.",
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
        detailHtml: "The network address and port for the forwarder's built-in status endpoint. Use <code>0.0.0.0</code> to allow access from other devices on the network, or <code>127.0.0.1</code> to restrict access to this device only.",
        default: "0.0.0.0:8080",
        range: "Valid IP:port combination",
        recommended: "0.0.0.0:8080 for standard setups. Use 127.0.0.1:8080 if you don't need to check status from other devices.",
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
        detailHtml: "Controls the forwarder's update behavior.<br><br><strong>Automatic</strong>: Checks for updates and downloads/applies them automatically. The service will restart to apply updates.<br><br><strong>Check Only</strong>: Checks for updates and notifies but does not download or apply them. Use this when you want to review updates before applying.<br><br><strong>Disabled</strong>: No update checking. Use this on race day to prevent unexpected restarts.",
        default: "Automatic",
        range: "Automatic, Check Only, Disabled",
        recommended: "Set to Disabled on race day to prevent unexpected restarts. Use Automatic or Check Only during setup.",
      },
    },
    tips: [
      "<strong>Set Update Mode to Disabled on race day</strong> to prevent unexpected service restarts during timing.",
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
        detailHtml: "A unique identifier automatically derived from the forwarder's authentication token (e.g. <code>fwd-3a9f1c2b8e4d07f1</code>). The ID is stable across restarts as long as the token doesn't change. The server uses this ID to identify the forwarder in the dashboard and receiver stream list.",
      },
      version: {
        label: "Version",
        summary: "Software version currently running on this forwarder.",
        detailHtml: "The version of the forwarder service currently running. Use this to confirm an update has been applied after a service restart.",
      },
      readiness: {
        label: "Readiness",
        summary: "Whether the forwarder has finished starting up and is operating normally.",
        detailHtml:
          "<ul>" +
          "<li><strong>Ready</strong>: The forwarder is collecting reads from configured readers and forwarding them when the server connection is available.</li>" +
          "<li><strong>Not ready</strong>: The forwarder is still starting up or encountered an initialization error. The reason is shown next to the badge. This is normal for a few seconds after the service starts. If it persists, check the log for errors.</li>" +
          "</ul>" +
          "Readiness does not depend on the server connection — a forwarder can be ready and collecting reads even while the server is unreachable.",
      },
    },
    tips: [
      "The Forwarder ID is tied to the authentication token. If you rotate the token, the ID will change and the server will treat this as a new forwarder.",
      "'Not ready' is expected for a few seconds after the service starts or restarts. If it persists, check the Logs section for errors.",
      "Readiness does not depend on the server connection. A forwarder can be ready and collecting reads even while disconnected — reads accumulate in the journal and are sent when the connection is restored.",
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
        detailHtml: "<strong>Connected</strong>: The forwarder is actively sending reads to the server.<br><br><strong>Disconnected</strong>: The forwarder is not currently connected. Reads continue to accumulate in the journal and will be sent automatically when the connection is restored.",
      },
      restart_needed: {
        label: "Restart Needed",
        summary: "Whether a saved configuration change is waiting to take effect.",
        detailHtml: "Shows <strong>Pending</strong> when a configuration change has been saved but not yet applied. The running service must restart to pick up the changes. Click <strong>Restart Now</strong> to apply.<br><br>Shows <strong>None</strong> when the running service reflects the current configuration.",
      },
    },
    tips: [
      "A disconnected uplink does not lose reads. The journal stores all reads and replays them once the connection recovers.",
      "If the uplink stays disconnected, verify the server URL and authentication token are correct, and that the server is reachable from the forwarder's network.",
      "'Restart Now' restarts the forwarder service, not the physical device. Readers will briefly disconnect and reconnect. No reads are lost.",
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
        summary: "Number of chip reads received from this reader since the forwarder last started.",
        detailHtml: "A running count of chip reads received from this reader since the forwarder service was last started. This counter resets to zero on each service restart. Use this to confirm reads are actively flowing from a reader during the current session.",
      },
      reads_total: {
        label: "Reads (Total)",
        summary: "Total chip reads from this reader recorded in the journal, across all sessions.",
        detailHtml: "The cumulative count of chip reads from this reader across all sessions. This count persists across service restarts when using a file-based journal. If using an in-memory journal, this matches the session count. Use this to gauge total throughput from a reader over the course of an event.",
      },
      local_port: {
        label: "Local Port",
        summary: "Port on this device where the reader's chip reads are available for local timing software.",
        detailHtml: "The forwarder makes reads from this reader available on this port, so timing software on the local network can receive them directly — independent of the server connection.<br><br>The port is either auto-calculated as <strong>10000 + the last octet of the reader's IP address</strong> (e.g. a reader at 192.168.0.50 uses port 10050), or a custom value if a Local Port Override is configured.",
      },
      last_seen: {
        label: "Last Seen",
        summary: "How long ago the most recent chip read was received from this reader.",
        detailHtml: "The time elapsed since the forwarder last received a chip read from this reader. Updates automatically while the page is open.<br><br>Shows <strong>never</strong> if no reads have been received in the current session. A rapidly increasing value while the reader is connected may indicate the timing mat is idle or no chips are in range.",
      },
      epoch_name: {
        label: "Epoch Name",
        summary: "Optional label for the current epoch on this reader, e.g. 'Race 1' or 'Wave 2'.",
        detailHtml: "Assigns a human-readable name to the reader's current epoch. The name is saved to the server and displayed as the active epoch label. Clearing the field and saving removes the name. The name applies to the current epoch only — after advancing to a new epoch, set a new name to identify it.",
      },
      advance_epoch: {
        label: "Advance Epoch",
        summary: "Starts a new epoch for this reader, separating subsequent reads from previous ones.",
        detailHtml: "Advances the reader's stream to a new epoch. All reads from this point forward are recorded under the new epoch, allowing the server and receiver to distinguish them from previous reads.<br><br>Reads already captured in earlier epochs are not deleted and will still be delivered if not yet received. Use this at the start of each race or wave to create a clean separation in the read stream. After advancing, set an epoch name to identify the new segment.",
      },
      clock_drift: {
        label: "Clock Drift",
        summary: "Difference between the reader's internal clock and the forwarder's local clock.",
        detailHtml:
          "Shows how far the reader's clock deviates from the forwarder's system clock. " +
          "A positive value means the reader clock is behind; a negative value means it is ahead. " +
          "The forwarder checks the reader clock automatically on connect and periodically during the session." +
          "<br><br>" +
          "The color indicates severity:" +
          "<ul>" +
          '<li><strong class="text-green-500">Green</strong> \u2014 less than 100\u2009ms. Normal; no action needed.</li>' +
          '<li><strong class="text-yellow-500">Yellow</strong> \u2014 100\u2009ms to 499\u2009ms. Noticeable drift; consider syncing before the next race.</li>' +
          '<li><strong class="text-red-500">Red</strong> \u2014 500\u2009ms or more. Significant drift that will affect timestamp accuracy; sync the clock now.</li>' +
          "</ul>" +
          "A dash (\u2014) means the reader clock has not been read yet in the current session." +
          "<br><br>" +
          "Use <strong>Sync Clock</strong> to correct the drift.",
      },
      tto_bytes: {
        label: "TTO Bytes",
        summary: "Adds extra metadata to each chip read for compatible timing software.",
        detailHtml:
          "TTO (Time To Own) is an IPICO reader feature that appends extra metadata to every chip read, including antenna index and pass-direction flags." +
          "<br><br>" +
          "<strong>Enabled</strong>: Each chip read includes antenna and pass-direction metadata." +
          "<br><br>" +
          "<strong>Disabled</strong>: Standard reads with no extra metadata. Compatible with all timing software." +
          "<br><br>" +
          "TTO is not required for normal race timing. Enable it only if your timing software uses TTO metadata.",
        default: "Disabled",
        recommended: "Leave disabled unless your timing software explicitly uses TTO metadata.",
      },
      sync_clock: {
        label: "Sync Clock",
        summary: "Synchronizes the reader's internal clock to the forwarder's local time.",
        detailHtml: "Sends the current time from the forwarder to the reader using a precision sync procedure. After the sync completes, the residual drift is reported in milliseconds.<br><br>Accurate chip-read timestamps depend on the reader's clock. Sync the clock before each race — especially after the reader has been powered on for the first time, after a long idle period, or if you notice timestamp anomalies. The button is only available while the reader is connected.",
        recommended: "Sync the clock at the start of each race day and again before each race if high timestamp accuracy is required.",
      },
      refresh_reader: {
        label: "Refresh",
        summary: "Re-polls the reader for its current status, configuration, and clock.",
        detailHtml: "Queries the reader and updates all displayed info: status, read mode, TTO setting, and clock. The forwarder polls this automatically on connect, but you can use Refresh at any time — for example, after changing settings directly on the reader, or to confirm a previous command took effect.",
      },
      recording: {
        label: "Start / Stop Recording",
        summary: "Toggles whether the reader is recording chip reads to its onboard storage.",
        detailHtml: "Controls the reader's onboard recording state." +
          "<ul>" +
          "<li><strong>Recording on</strong>: The reader stores each chip read in its internal memory in addition to streaming reads live to the forwarder.</li>" +
          "<li><strong>Recording off</strong>: The reader streams reads but does not save them to onboard storage.</li>" +
          "</ul>" +
          "Onboard recording is independent of the live data stream — reads are forwarded to the server regardless. Use recording as a safety net: if the forwarder loses its connection mid-race, reads are preserved on the reader and can be retrieved later with <strong>Download Reads</strong>.",
        recommended: "Turn recording on before each race as a safety net. Download and clear records after each event.",
      },
      download_reads: {
        label: "Download Reads",
        summary: "Downloads all chip reads stored in the reader's onboard memory to the forwarder.",
        detailHtml: "Transfers all records stored in the reader's onboard memory to the forwarder. A progress bar shows the download status. Downloaded reads are delivered to the server just like live reads.<br><br>This is the primary recovery path after a connection outage: if the forwarder lost its connection during a race, use Download Reads once connectivity is restored to retrieve any reads that were captured to onboard storage.<br><br>Only one download can run at a time per reader. After a successful download, use <strong>Clear Records</strong> to free the reader's storage for the next race.",
      },
      clear_records: {
        label: "Clear Records",
        summary: "Erases all stored records from the reader's onboard memory.",
        detailHtml: "Permanently erases all chip reads stored in the reader's internal memory. This takes approximately 10 seconds to complete.<br><br><strong>This action is irreversible.</strong> Always use <strong>Download Reads</strong> first if you need to recover the stored data before clearing. Clear records after each event to ensure the reader's storage is ready for the next race.",
        recommended: "Always download reads before clearing. Clear records at the end of each race day.",
      },
    },
    tips: [
      "Use 'Advance Epoch' at the start of each race or wave to cleanly separate reads in the data stream.",
      "If Reads (Session) stops increasing while the reader is connected, check that chips are in range of the timing mat and that the reader is in the correct read mode.",
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
