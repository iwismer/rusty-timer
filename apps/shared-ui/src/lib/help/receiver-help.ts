import type { HelpContext } from './help-types';

export const RECEIVER_HELP = {
  config: {
    title: 'Receiver Configuration',
    overview:
      'Core connection settings for the receiver. These determine how the receiver identifies itself and reaches the server coordinator.',
    fields: {
      receiver_id: {
        label: 'Receiver ID',
        summary: 'Unique identifier for this receiver instance.',
        detailHtml:
          "A unique string that identifies this receiver endpoint. Use a descriptive ID like 'finish-line-pc' or 'timing-tent-a' so operators can identify which receiver is which.",
        default: 'None (required)',
        recommended:
          'Use a short, descriptive name that identifies the physical location or purpose of this receiver.',
      },
      server_url: {
        label: 'Server URL',
        summary: 'URL of the server coordinator.',
        detailHtml:
          'The full URL of the server to connect to (e.g. <code>https://server.example.com</code>). The receiver uses it for endpoint discovery, allow-list state, and status updates; chip reads flow directly over P2P sessions.',
        default: 'None (required)',
        recommended: 'Use HTTPS in production.',
      },
      token: {
        label: 'Token',
        summary: 'Authentication token for server M2M calls.',
        detailHtml:
          'The authentication token used by this receiver when it calls server registration, status, and announcer endpoints.',
        default: 'None (required)',
      },
    },
    tips: [
      'Save your config before connecting for the first time.',
      "If the receiver can't connect, verify the server URL, token, and allow-list entry for this endpoint.",
    ],
    seeAlso: [{ sectionKey: 'receiver_mode', label: 'Receiver Mode' }],
  },
  receiver_mode: {
    title: 'Receiver Mode',
    overview:
      'The receiver mode determines how streams are subscribed and how epoch controls behave. Choose the mode that matches your timing workflow.',
    fields: {
      mode: {
        label: 'Mode',
        summary: 'Operating mode for stream subscriptions.',
        detailHtml:
          '<strong>Live</strong> mode auto-subscribes to available streams. New streams are added automatically as forwarders connect. This is the default for standard race timing.<br><br>' +
          'To re-send historical data to your timing software (for example after a crash), use the per-stream <strong>From epoch</strong> control together with <strong>Admin &gt; Reset local stream data</strong> — see the Streams tab help.',
        default: 'Live',
        range: 'Live',
        recommended: 'Use Live mode for standard race timing.',
      },
    },
    tips: [
      'Use Live mode for standard race timing. It auto-subscribes to all available streams.',
      'Changing modes takes effect immediately. Active subscriptions may change.',
    ],
    seeAlso: [
      { sectionKey: 'streams', label: 'Available Streams' },
      { sectionKey: 'config', label: 'Receiver Configuration' },
    ],
  },
  streams: {
    title: 'Available Streams',
    overview:
      'Streams represent data feeds from forwarder/reader pairs. Each stream delivers chip reads from one reader to the receiver, which forwards them to your timing software.',
    fields: {
      stream_identity: {
        label: 'Stream',
        summary:
          'A stream is identified by its forwarder ID and reader IP address; the dot shows its live delivery status.',
        detailHtml:
          'Each stream represents a unique data feed from a specific reader on a specific forwarder. Streams are identified by the combination of forwarder ID and reader IP. If the forwarder has a display name set, it is shown instead of the ID.<br><br>' +
          'The dot next to each stream reflects its current state:' +
          '<ul>' +
          '<li><strong>Green</strong>: reads are flowing or the stream is healthy.</li>' +
          '<li><strong>Yellow</strong>: subscribing, waiting for data, or the reader needs attention.</li>' +
          '<li><strong>Red</strong>: subscribed but not receiving data.</li>' +
          '<li><strong>Gray</strong>: not subscribed.</li>' +
          '</ul>' +
          'A badge such as <strong>Reader down</strong> or <strong>Waiting for data</strong> appears next to the stream name when attention is needed.',
      },
      subscribed: {
        label: 'Subscribed',
        summary: 'Whether the receiver is actively receiving data from this stream.',
        detailHtml:
          "A subscribed stream actively delivers chip reads to the receiver's local port. Unsubscribing stops local delivery but does <strong>not</strong> stop the forwarder from journaling data. Data continues to accumulate on the forwarder and can be replayed later.",
      },
      last_read: {
        label: 'Last Read',
        summary: 'Most recent chip read on this stream.',
        detailHtml:
          'Shows the timestamp of the latest chip read, plus the participant bib and name when participant and chip data have been imported. If the chip cannot be matched, an unknown-participant or unknown-chip label is shown instead. Import participant and chip files on the Announcer tab to resolve names.',
      },
      reads: {
        label: 'Reads',
        summary: 'Total chip reads received on this stream.',
        detailHtml:
          'A running count of chip reads delivered to the receiver for this stream. Only shown for subscribed streams. If the count is not increasing while chips are crossing the reader, check the stream status dot and the forwarder connection.',
      },
      local_port: {
        label: 'Local Port',
        summary: 'The local port where reads from this stream are forwarded.',
        detailHtml:
          "The port on this machine where the receiver forwards chip reads from this stream. Your timing software should be configured to listen on this port.<br><br>For standard reader ports, the default is calculated as <strong>10000 + the last octet of the reader's IP address</strong>. Readers using non-standard source ports get a deterministic fallback port. Custom ports can be set via <strong>Admin &gt; Port Overrides</strong>.",
      },
      stream_epoch: {
        label: 'Epoch',
        summary: 'The current epoch (data segment) the stream is reading from.',
        detailHtml:
          "An epoch represents a segment of timing data, typically corresponding to a race or wave. Epochs are numbered sequentially. The epoch name (if set) provides a human-readable label like 'Race 1' or 'Wave A'.",
      },
      stream_metrics: {
        label: 'Stream Metrics',
        summary: 'Detailed read counters, shown when a stream row is expanded.',
        detailHtml:
          'Click a stream row to expand its metrics, split into <strong>Lifetime</strong> and <strong>Current Epoch</strong>:' +
          '<ul>' +
          '<li><strong>Raw count</strong>: Total frames received, including retransmits.</li>' +
          '<li><strong>Dedup count</strong>: Unique frames after deduplication.</li>' +
          '<li><strong>Retransmit</strong>: Duplicate frames that matched existing events.</li>' +
          '<li><strong>Lag</strong>: Forwarder-reported delay since the last unique frame.</li>' +
          '<li><strong>Unique chips</strong>: Distinct chip IDs detected in the current epoch.</li>' +
          '<li><strong>Last read / Time since last read</strong>: When the last unique frame arrived, and a live-updating elapsed time.</li>' +
          '</ul>',
      },
      event_type: {
        label: 'Event Type',
        summary: "Whether this stream's reads are written as Start or Finish rows.",
        detailHtml:
          "When DBF output is enabled, each stream's reads are written as either <strong>Start</strong> or <strong>Finish</strong> rows for your timing software. Set this to match the reader's physical location.",
        default: 'Finish',
        range: 'Start, Finish',
      },
      earliest_epoch: {
        label: 'From Epoch',
        summary: 'Skip older epochs and only fetch reads from a chosen epoch onward.',
        detailHtml:
          'Sets an earliest-epoch override: the receiver only fetches reads from the chosen epoch onward. The override applies when the stream (re)subscribes; data already received locally is unaffected.<br><br>' +
          '<strong>Important:</strong> the skip is permanent for this receiver — clearing the override later does <em>not</em> back-fill the skipped reads. If the chosen epoch is no longer available on the forwarder, the stream pauses (shown as “paused: epoch unavailable”) rather than delivering older data; clear the override to resume.<br><br>' +
          '<strong>Recovery recipe</strong> (re-send one race to your timing software after a crash): set <strong>From epoch</strong> to the race’s epoch, run <strong>Admin &gt; Reset local stream data</strong> for that stream, and reconnect your timing software to the local port. It will receive only the chosen epoch onward.',
      },
      announce: {
        label: 'Announce',
        summary: "Publish this stream's reads to the announcer board.",
        detailHtml:
          'When checked, reads from this stream are published to the server announcer board. Announcer publishing must also be turned on globally on the <strong>Announcer</strong> tab for rows to appear.',
        default: 'Off',
      },
      subscribe_all: {
        label: 'Subscribe All',
        summary: 'Subscribe to every available stream at once.',
        detailHtml:
          'Subscribes to all streams currently listed as <strong>Available</strong>. This is equivalent to pressing Subscribe on each stream individually.',
      },
    },
    tips: [
      'Click a stream row to expand it and see detailed metrics, epoch controls, and per-stream actions.',
      'Unsubscribing a stream only stops local delivery. Data continues to accumulate on the forwarder and can be replayed later.',
      "If your timing software isn't receiving reads, check that it's listening on the correct local port.",
      "The 'degraded' indicator means the receiver reported a local issue with this stream. Reads may still flow; check the receiver logs and forwarder status page.",
      "Use <strong>Admin &gt; Port Overrides</strong> to customize which local port each stream uses if the defaults don't match your timing software setup.",
    ],
    seeAlso: [
      { sectionKey: 'receiver_mode', label: 'Receiver Mode' },
      { sectionKey: 'announcer', label: 'Announcer' },
    ],
  },
  connections: {
    title: 'Connections',
    overview:
      "Shows the receiver's link to the server and to each forwarder, with controls to connect, disconnect, and manage forwarder readers remotely.",
    fields: {
      server_status: {
        label: 'Server',
        summary: 'Whether the server coordinator is reachable and this receiver is approved.',
        detailHtml:
          "The server card shows reachability and this receiver's approval state. A new receiver shows <strong>Waiting for server approval</strong> until an administrator approves its endpoint on the server. Chip reads do not flow through the server; it handles discovery, allow-lists, and status.",
      },
      open_admin_panel: {
        label: 'Open Admin Panel',
        summary: "Opens the server's admin page in your browser.",
        detailHtml:
          'Opens the server admin panel, where administrators approve receiver endpoints and manage the allow-list. Available once a server URL is configured.',
      },
      forwarder_state: {
        label: 'Forwarder State',
        summary: 'Connection state of each forwarder.',
        detailHtml:
          'Each forwarder card shows its P2P connection state:' +
          '<ul>' +
          '<li><strong>Connecting…</strong>: a connection attempt is in progress.</li>' +
          '<li><strong>Subscribed</strong>: connected and delivering at least one stream.</li>' +
          '<li><strong>Connected</strong>: P2P session established, no streams subscribed yet.</li>' +
          '<li><strong>Unavailable</strong>: the forwarder cannot be reached. Try <strong>Reconnect</strong>.</li>' +
          '<li><strong>Disconnected</strong>: not connected. Use <strong>Connect</strong> to establish a session.</li>' +
          '</ul>' +
          "The subscribed/available counts show how many of the forwarder's streams you are receiving.",
      },
      forwarder_actions: {
        label: 'Connect / Disconnect / Reconnect',
        summary: 'Manually control the P2P session to a forwarder.',
        detailHtml:
          '<strong>Connect</strong> establishes a P2P session to a disconnected forwarder. <strong>Disconnect</strong> closes the session; the forwarder keeps journaling reads, so data can be replayed later. <strong>Reconnect</strong> drops and re-establishes the session, which is the first thing to try when a forwarder shows as Unavailable.',
      },
      forwarder_configure: {
        label: 'Configure',
        summary: "Edit a forwarder's settings remotely from this receiver.",
        detailHtml:
          'Opens the remote configuration dialog for the forwarder. Only shown when the forwarder supports remote configuration.',
      },
      forwarder_battery: {
        label: 'Battery',
        summary: 'UPS battery level reported by the forwarder.',
        detailHtml:
          'When the forwarder has a UPS configured, its battery percentage and charging state are shown here and next to its streams. A forwarder running on battery may lose power; check it before it goes offline.',
      },
      reader_controls: {
        label: 'Reader Controls',
        summary: "Manage the forwarder's IPICO readers remotely.",
        detailHtml:
          "When a forwarder supports reader control, each of its readers appears with a control panel. From here you can sync the reader clock, name or advance the epoch, change the read mode, toggle recording, and download stored records — the same controls available in the forwarder's own UI.",
      },
    },
    tips: [
      "If a forwarder shows Unavailable, try Reconnect first, then check the forwarder's power and network.",
      'Disconnecting a forwarder does not delete data — the forwarder keeps journaling reads, and they can be replayed after reconnecting.',
      "If the server shows 'Waiting for approval', an administrator must approve this receiver's endpoint in the server admin panel.",
    ],
    seeAlso: [
      { sectionKey: 'config', label: 'Receiver Configuration' },
      { sectionKey: 'streams', label: 'Available Streams' },
    ],
  },
  rd_import: {
    title: 'Race Director Import',
    overview:
      'Pull participant and chip data from a Race Director working folder. The receiver polls the Race Director DBF files and uses the data to resolve chip reads to bibs and names.',
    fields: {
      rd_import_enabled: {
        label: 'Poll Race Director Files',
        summary: 'Enables periodic polling of Race Director DBF files.',
        detailHtml:
          'When enabled, the receiver periodically reads Race Director participant and chip files from the configured folder. Files are only re-parsed when they change, and a failed parse keeps the previous data so participant data is not blanked mid-race.',
        default: 'Disabled',
        recommended:
          'Enable when Race Director is running on this machine or the folder is reachable over a network share.',
      },
      rd_import_dir: {
        label: 'Folder',
        summary: 'The Race Director working folder containing the DBF files.',
        detailHtml:
          'Path to the Race Director working folder. The same folder is also used for the <code>IPICO.DBF</code> output file when sending reads to Race Director.',
        default: 'C:\\Winrace\\Files',
        recommended: "Use Race Director's working folder so import and output stay in sync.",
      },
      rd_import_interval: {
        label: 'Poll Interval',
        summary: 'How often the receiver checks the Race Director files for changes.',
        detailHtml:
          'The receiver checks the file timestamps at this interval and only re-parses when something changed, so frequent polling is inexpensive. A shorter interval picks up participant edits sooner.',
        default: '15 seconds',
        range: '1 second or more',
        recommended: 'The 15-second default is fine for most events.',
      },
    },
    tips: [
      'If an import fails because Race Director is mid-write, the receiver keeps the last good data and retries on the next poll.',
      'The folder must exist before enabling the import.',
    ],
    seeAlso: [{ sectionKey: 'dbf_output', label: 'Race Director Output' }],
  },
  dbf_output: {
    title: 'Race Director Output',
    overview:
      'Write received chip reads to an Ipico Direct DBF file that Race Director reads. Streams assigned to DBF reader slots write into a single shared file.',
    fields: {
      dbf_enabled: {
        label: 'Write Reads to DBF',
        summary: 'Enables writing received reads to the IPICO.DBF file for Race Director.',
        detailHtml:
          'When enabled, the receiver appends received chip reads to <code>IPICO.DBF</code> inside the Race Director folder. Streams assigned to DBF reader slots share this one file; the IPICO DBF format supports a limited set of reader slots. Race Director reads it as an Ipico Direct source.',
        default: 'Disabled',
      },
      dbf_flush_interval: {
        label: 'DBF Write Interval',
        summary: 'How often new reads are written to the DBF file.',
        detailHtml:
          'New reads are buffered and written to the file at this interval. A shorter interval gets reads into Race Director sooner; a longer interval reduces file writes.',
        default: '1 second',
        range: '0.25-5 seconds',
        recommended: 'Use the 1-second default unless Race Director needs faster updates.',
      },
      clear_dbf: {
        label: 'Clear DBF File',
        summary: 'Empties the DBF file and regenerates it from stored reads.',
        detailHtml:
          "Removes all records from <code>IPICO.DBF</code> and marks stored reads for re-delivery. On the next write pass the file is rebuilt from the receiver's durable store, so no reads are lost.<br><br>Use this if the file is corrupted or Race Director needs a fresh copy of all reads.",
      },
    },
    tips: [
      'The DBF file is written to the folder configured under Race Director Import.',
      "Clearing the DBF file does not delete stored reads — the file is regenerated from the receiver's durable store.",
      "If Race Director isn't seeing reads, verify the folder path and that DBF output is enabled and saved.",
    ],
    seeAlso: [{ sectionKey: 'rd_import', label: 'Race Director Import' }],
  },
  announcer: {
    title: 'Announcer',
    overview:
      'Publishes selected stream reads to the server announcer board and manages the participant and chip data used to show bibs and names.',
    fields: {
      announcer_enabled: {
        label: 'Announcer Publishing',
        summary: 'Master switch for publishing reads to the server announcer board.',
        detailHtml:
          'When on, subscribed streams that have <strong>Announce</strong> checked publish reads to the server announcer board. Choose which streams publish on the <strong>Streams</strong> tab. When off, nothing is published regardless of per-stream settings.',
        default: 'Off',
      },
      max_list_size: {
        label: 'Max Finishers Shown',
        summary: 'Caps how many rows the server announcer feed keeps visible.',
        detailHtml:
          'Limits the number of finisher rows visible on the server announcer page. Older rows scroll off as new finishers arrive.',
        range: '1-500',
      },
      open_announcer_page: {
        label: 'Open Announcer Page',
        summary: 'Opens the server announcer board in your browser.',
        detailHtml:
          'Opens the announcer page hosted by the server, where published reads appear with bib and name.',
      },
      participants_file: {
        label: 'Participants (.ppl)',
        summary: 'Import a participant file so announcer rows show names.',
        detailHtml:
          'Imports a participant file (<code>.ppl</code>, <code>.csv</code>, or <code>.txt</code>) mapping bib numbers to participant names. Each import <strong>replaces</strong> all existing participant data.',
      },
      chips_file: {
        label: 'Chip Assignments (.bibchip)',
        summary: 'Import a chip-assignment file mapping chip IDs to bibs.',
        detailHtml:
          'Imports a chip-assignment file (<code>.bibchip</code>, <code>.csv</code>, or <code>.txt</code>) mapping chip IDs to bib numbers. Combined with the participant file, this lets announcer rows and the Streams tab show bib and name for each read. Each import <strong>replaces</strong> all existing chip data.',
      },
      data_stats: {
        label: 'Data Stats',
        summary: 'Counts of imported participants and chips, and how well they match.',
        detailHtml:
          'After importing, the stats panel shows:' +
          '<ul>' +
          '<li><strong>Participants / Chips</strong>: Total rows imported from each file.</li>' +
          '<li><strong>Matched (bib + chip)</strong>: Participants with an assigned chip.</li>' +
          '<li><strong>Participants missing chips</strong>: Participants with no chip assignment.</li>' +
          '<li><strong>Chips with no participant</strong>: Chips whose bib has no participant entry.</li>' +
          '</ul>',
      },
      rd_auto_import: {
        label: 'Race Director Auto Import',
        summary: 'Automatically imports participant and chip data from a Race Director directory.',
        detailHtml:
          'When enabled in Race Director Import settings, the receiver watches the configured folder and imports participant and chip data automatically. Manual imports are unnecessary unless you want to replace the auto-imported data.',
      },
    },
    tips: [
      'Import both a participant file and a chip-assignment file — announcer rows need both to show bib and name.',
      "A high 'Participants missing chips' count usually means the chip file is stale or from a different event.",
      'Announcer publishing is two switches: the global toggle here, plus the per-stream Announce checkbox on the Streams tab.',
    ],
    seeAlso: [{ sectionKey: 'streams', label: 'Available Streams' }],
  },
  status_bar: {
    title: 'Status Bar',
    overview:
      'The bar along the bottom of the window summarizes overall connection health and total read volume.',
    fields: {
      overall_health: {
        label: 'Health Indicator',
        summary: 'Colored dot summarizing all connections.',
        detailHtml:
          'A single at-a-glance indicator: <strong>green</strong> means all connections are healthy, <strong>yellow</strong> means some connections are degraded, and <strong>red</strong> means the receiver is disconnected. Check the Connections tab for details when it is not green.',
      },
      total_reads: {
        label: 'Reads Counter',
        summary: 'Total chip reads across all subscribed streams.',
        detailHtml:
          'The sum of read counts from every subscribed stream. A steadily increasing count during a race is a quick sanity check that data is flowing.',
      },
      identity_version: {
        label: 'Receiver ID & Version',
        summary: "This receiver's ID and the installed app version.",
        detailHtml:
          'Shows the configured receiver ID and the application version. An update arrow appears when a new version is available; click it for update details.',
      },
    },
    seeAlso: [{ sectionKey: 'connections', label: 'Connections' }],
  },
} as const satisfies HelpContext;
