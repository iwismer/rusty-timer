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
        detailHtml: "The full URL of the server to connect to (e.g. <code>wss://server.example.com:8080</code>). The receiver maintains a persistent connection to the server for real-time data streaming. Use <code>wss://</code> for encrypted connections.",
        default: "None (required)",
        recommended: "Use wss:// for production to encrypt data in transit.",
      },
      token: {
        label: "Token",
        summary: "Authentication token for connecting to the server.",
        detailHtml: "The authentication token used to connect this receiver to the server. It must match a valid token configured on the server.",
        default: "None (required for authenticated servers)",
      },
    },
    tips: [
      "Save your config before connecting for the first time.",
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
        detailHtml: "The receiver supports three operating modes:" +
          "<ul>" +
          "<li><strong>Live</strong>: Auto-subscribes to all available streams. New streams are added automatically as forwarders connect. This is the default for standard race timing.</li>" +
          "<li><strong>Race</strong>: Follows server-defined stream assignments for a selected race. Use this when the server operator has set up race definitions.</li>" +
          "<li><strong>Targeted Replay</strong>: Allows per-stream epoch selection for replaying historical data. Use this to re-send timing data to your timing software, for example after a crash.</li>" +
          "</ul>",
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
  races: {
    title: "Races",
    overview:
      "Races organize participant and chip data for an event. Create a race, then upload participants and chip mappings so chip reads can resolve to bib numbers and names.",
    fields: {
      create_race: {
        label: "Create Race",
        summary:
          "Creates a new race to hold participant and chip-mapping data.",
        detailHtml:
          "Enter a name and click <strong>Create Race</strong> to add a new race. " +
          "Once created, open the race to upload participant and chip-mapping files." +
          "<br><br>" +
          "Each race has its own independent set of participants and chip mappings, so you can run multiple races on the same day without data overlap.",
      },
      delete_race: {
        label: "Delete Race",
        summary:
          "Permanently deletes the race and all its participants and chip mappings. Any forwarder assigned to this race will be unassigned.",
        detailHtml:
          "<strong>This action is irreversible.</strong> Deleting a race removes all of its participant data and chip mappings. Forwarders assigned to this race will be unassigned. " +
          "Timing data (reads/events) is not affected — only the race metadata is deleted." +
          "<br><br>" +
          "A race cannot be deleted while a receiver session is actively using it." +
          "<br><br>" +
          "You will be asked to confirm before the delete proceeds.",
      },
    },
    tips: [
      "Create one race per start. For multi-wave events, you can use a single race if all waves share the same participant list, or separate races per wave.",
      "Upload participants and chip mappings before race day if possible — you can always update them later.",
      "Deleting a race does not delete any timing data. Reads are preserved regardless of race assignments.",
    ],
    seeAlso: [
      { sectionKey: "race_detail", label: "Race Detail" },
      { sectionKey: "receiver_mode", label: "Receiver Mode" },
    ],
  },
  race_detail: {
    title: "Race Detail",
    overview:
      "Manage participant and chip-mapping data for a specific race. Upload files to populate participants and map chips to bib numbers.",
    fields: {
      upload_participants: {
        label: "Upload Participants (.ppl)",
        summary:
          "Upload a .ppl file containing participant data: bib number, name, gender, and team.",
        detailHtml:
          "Upload a <code>.ppl</code> file to import participant data for this race. The file should contain bib numbers, first and last names, gender, and optionally a team/affiliation." +
          "<br><br>" +
          "Uploading replaces all existing participants for this race. If you need to update the list, upload the corrected file again." +
          "<br><br>" +
          "Participants are used together with chip mappings to resolve raw chip reads into names and bib numbers.",
      },
      upload_chips: {
        label: "Upload Chip Mappings (.bibchip)",
        summary:
          "Upload a file mapping chip IDs to bib numbers, enabling chip-read resolution.",
        detailHtml:
          "Upload a <code>.bibchip</code>, <code>.csv</code>, or <code>.txt</code> file that maps chip IDs to bib numbers. Each line should contain a bib number and the corresponding chip ID." +
          "<br><br>" +
          "This mapping is what allows the system to show participant names instead of raw chip IDs when a read comes in. Without chip mappings, reads display as hex chip IDs only." +
          "<br><br>" +
          "Uploading replaces all existing chip mappings for this race.",
      },
      unmatched_chips: {
        label: "Unmatched Chips",
        summary:
          "Chip mappings that reference bib numbers not found in the participant list.",
        detailHtml:
          "Shows chip-to-bib mappings where the bib number does not match any uploaded participant. This usually means:" +
          "<ul>" +
          "<li>The participant list is outdated or incomplete — re-upload it</li>" +
          "<li>The chip mapping file has stale bib numbers — re-upload it</li>" +
          "<li>A participant was added to the chip file but not the participant file</li>" +
          "</ul>" +
          "Unmatched chips will still record reads, but those reads won't resolve to a participant name.",
      },
    },
    tips: [
      "Upload chip mappings after participants so you can immediately see any unmatched bibs.",
      "If you see unmatched chips, check that the participant and chip files use the same bib numbering.",
      "You can re-upload files at any time to update the data — the new upload fully replaces the old data.",
    ],
    seeAlso: [{ sectionKey: "races", label: "Races" }],
  },
} as const satisfies HelpContext;
