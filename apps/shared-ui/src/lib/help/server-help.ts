import type { HelpContext } from "./help-types";

export const SERVER_HELP = {
  stream_filters: {
    title: "Stream Filters",
    overview:
      "Filter and organize the stream list to focus on the forwarders and readers relevant to your current race. Filters let you narrow the view by race assignment and connection status.",
    fields: {
      race_filter: {
        label: "Filter Streams by Race",
        summary:
          "Narrows the stream list to show only readers assigned to a specific race.",
        detailHtml:
          "Filters the stream list so only forwarders and readers assigned to the selected race are shown. " +
          "Select a race name to focus on that race's readers, or choose <strong>All races</strong> to see every stream." +
          "<br><br>" +
          "This filter works together with the per-forwarder race assignment — only forwarders whose race selection matches the chosen race will appear.",
        default: "All races",
      },
      hide_offline: {
        label: "Hide Offline",
        summary:
          "Hides streams whose forwarder is currently disconnected from the server.",
        detailHtml:
          "When checked, forwarders that are not currently connected are hidden from the stream list. " +
          "This keeps the view clean during active timing so you can focus on the equipment that is live and reporting reads." +
          "<br><br>" +
          "Offline forwarders are not lost — they reappear automatically once they reconnect, or you can uncheck this filter to see them.",
        default: "Unchecked (all streams shown)",
        recommended:
          "Enable during active racing to reduce clutter. Disable during setup to verify all forwarders are accounted for.",
      },
      forwarder_race: {
        label: "Forwarder Race Selection",
        summary:
          "Assigns a forwarder and all its readers to a specific race for chip-read resolution and filtering.",
        detailHtml:
          "Sets which race this forwarder belongs to. When a race is selected, the dashboard uses that race's participant and chip data to resolve chip reads into bib numbers and names." +
          "<br><br>" +
          "This assignment also controls which forwarders appear when using the <strong>Filter Streams by Race</strong> dropdown at the top of the page." +
          "<br><br>" +
          "If no race is selected, chip reads from this forwarder are shown as raw chip IDs without participant resolution.",
        default: "None (no race assigned)",
        recommended:
          "Assign the correct race before timing begins so chip reads resolve to participant names immediately.",
      },
    },
    tips: [
      "Assign each forwarder to its race before the start so chip reads display participant names right away.",
      "Use <strong>Hide Offline</strong> during active timing to keep the stream list focused on live equipment.",
      "If a forwarder covers multiple races during the day, update its race selection between races to switch participant data.",
    ],
    seeAlso: [{ sectionKey: "races", label: "Races" }],
  },
  races: {
    title: "Races",
    overview:
      "Races organize participant and chip data for an event. Create a race, upload participants and chip mappings, then assign forwarders to it so chip reads resolve to bib numbers and names on the dashboard.",
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
          "Permanently deletes the race and all its participants, chip mappings, and forwarder associations.",
        detailHtml:
          "<strong>This action is irreversible.</strong> Deleting a race removes all of its participant data, chip mappings, and any forwarder-race associations. " +
          "Timing data (reads/events) is not affected — only the race metadata is deleted." +
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
      { sectionKey: "stream_filters", label: "Stream Filters" },
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
          "Participants are used together with chip mappings to resolve raw chip reads into names and bib numbers on the dashboard and announcer page.",
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
  announcer: {
    title: "Announcer Configuration",
    overview:
      "The announcer shows a live feed of recent chip reads resolved to participant names — typically displayed on a big screen at the finish line. Configure which streams feed the announcer and how many reads to show.",
    fields: {
      enabled: {
        label: "Enable Announcer",
        summary:
          "Turns the announcer feed on or off.",
        detailHtml:
          "When enabled, the server pushes recent chip reads to the announcer display page. The announcer resolves chip IDs to participant names using the race data assigned to each forwarder." +
          "<br><br>" +
          "Disable the announcer when it's not needed to reduce server load.",
        default: "Disabled",
      },
      streams: {
        label: "Streams",
        summary:
          "Selects which reader streams feed into the announcer display.",
        detailHtml:
          "Check the streams whose reads should appear on the announcer display. At least one stream must be selected when the announcer is enabled." +
          "<br><br>" +
          "Typically you'll select the finish-line reader(s). For multi-point timing, you may want to include intermediate split-point readers as well.",
        recommended:
          "Select only the finish-line stream(s) for a clean announcer display.",
      },
      max_list_size: {
        label: "Max List Size",
        summary:
          "Maximum number of recent reads shown on the announcer display.",
        detailHtml:
          "Controls how many reads appear on the announcer page at once. Older reads scroll off as new ones arrive. A larger value shows more history; a smaller value keeps the display focused on the most recent finishers.",
        default: "25",
        range: "1–500",
        recommended: "25–50 for a finish-line display. Increase for high-volume events.",
      },
      reset: {
        label: "Reset Announcer",
        summary:
          "Clears the announcer's current read list and starts fresh.",
        detailHtml:
          "Clears all reads currently shown on the announcer display. The announcer will start collecting new reads from scratch." +
          "<br><br>" +
          "Use this between races to clear the previous race's finishers from the display before the next race starts.",
        recommended: "Reset between each race to start with a clean display.",
      },
    },
    tips: [
      "Reset the announcer between races to clear the display for the next group of finishers.",
      "Make sure the forwarder feeding the announcer is assigned to the correct race — otherwise chip reads won't resolve to names.",
      "Open the announcer page (<strong>Open announcer page</strong> link) in a separate browser window and display it full-screen on a projector or TV.",
    ],
    seeAlso: [
      { sectionKey: "races", label: "Races" },
      { sectionKey: "stream_filters", label: "Stream Filters" },
    ],
  },
  sbc_identity: {
    title: "Device Identity",
    overview:
      "Core identity and access settings for the SBC device. These values are used during first boot to configure the device.",
    fields: {
      hostname: {
        label: "Hostname",
        summary:
          "Network hostname for the SBC device. Auto-increments with 'Save & Next Device'.",
        detailHtml:
          "The hostname identifies the device on the network and in the server dashboard. Use a naming scheme that indicates the device's role (e.g. <code>rt-fwd-01</code>, <code>rt-fwd-02</code>)." +
          "<br><br>" +
          "When using <strong>Save & Next Device</strong>, the trailing number is automatically incremented for the next device.",
        default: "rt-fwd-01",
        recommended: "Use a consistent naming pattern like rt-fwd-01, rt-fwd-02, etc.",
      },
      admin_username: {
        label: "Admin Username",
        summary:
          "Linux user account created on the SBC for remote access.",
        detailHtml:
          "The admin user created during first-boot provisioning. This is the account you'll use to SSH into the device for maintenance or troubleshooting.",
        default: "rt-admin",
      },
      ssh_public_key: {
        label: "SSH Public Key",
        summary:
          "SSH public key for passwordless login to the device.",
        detailHtml:
          "Paste your SSH public key here (the contents of <code>~/.ssh/id_ed25519.pub</code> or similar). This key is installed on the device during first boot, allowing passwordless SSH access." +
          "<br><br>" +
          "Password authentication is disabled by default for security — you must provide a key.",
      },
    },
    tips: [
      "Use the same SSH key across all SBC devices so you can manage them all from one machine.",
      "Label each physical device with its hostname so you can identify it at the venue.",
    ],
    seeAlso: [
      { sectionKey: "sbc_network", label: "Network Configuration" },
      { sectionKey: "sbc_forwarder", label: "Forwarder Setup" },
    ],
  },
  sbc_network: {
    title: "Network Configuration",
    overview:
      "Network settings applied during first boot. These configure the SBC's static IP address, gateway, DNS, and optional Wi-Fi connection.",
    fields: {
      static_ipv4: {
        label: "Static IPv4/CIDR",
        summary:
          "Static IP address with subnet mask in CIDR notation (e.g. 192.168.1.51/24).",
        detailHtml:
          "The static IP address assigned to the SBC's Ethernet interface, with the subnet mask in CIDR notation. Auto-increments when using <strong>Save & Next Device</strong>." +
          "<br><br>" +
          "Make sure each device gets a unique IP on the network. Plan your IP range in advance for all devices you'll deploy.",
        default: "192.168.1.51/24",
        recommended: "Reserve a range of IPs for SBC devices (e.g. .51–.60) to avoid conflicts with other equipment.",
      },
      gateway: {
        label: "Gateway",
        summary:
          "Default gateway IP address, usually the router.",
        detailHtml:
          "The IP address of the network gateway (router). The SBC routes all non-local traffic through this address.",
        default: "192.168.1.1",
      },
      dns_servers: {
        label: "DNS Servers",
        summary:
          "Comma-separated list of DNS server IP addresses.",
        detailHtml:
          "One or more DNS server addresses, separated by commas. Used for resolving hostnames (e.g. the timing server URL).",
        default: "8.8.8.8, 8.8.4.4",
      },
      wifi_enabled: {
        label: "Enable Wi-Fi",
        summary:
          "Enables Wi-Fi connectivity on the SBC in addition to Ethernet.",
        detailHtml:
          "When enabled, the SBC will connect to the specified Wi-Fi network during boot. Wi-Fi is optional — Ethernet is the primary and recommended connection for reliability." +
          "<br><br>" +
          "Wi-Fi can serve as a backup connection or for venues where running Ethernet cable isn't practical.",
        default: "Disabled",
        recommended: "Use Ethernet as the primary connection. Only enable Wi-Fi if needed for your venue layout.",
      },
      wifi_ssid: {
        label: "Wi-Fi SSID",
        summary: "The Wi-Fi network name to connect to.",
        detailHtml: "The name (SSID) of the Wi-Fi network. Required when Wi-Fi is enabled.",
      },
      wifi_password: {
        label: "Wi-Fi Password",
        summary: "Password for the Wi-Fi network.",
        detailHtml: "The WPA/WPA2 password for the Wi-Fi network. Leave empty for open networks.",
      },
      wifi_country: {
        label: "Wi-Fi Country Code",
        summary:
          "Two-letter country code for Wi-Fi regulatory compliance.",
        detailHtml:
          "The ISO 3166-1 alpha-2 country code (e.g. <code>US</code>, <code>CA</code>, <code>GB</code>). Required for the Wi-Fi radio to operate on the correct channels and power levels for your region.",
        default: "CA",
      },
    },
    tips: [
      "Use Ethernet whenever possible — it's more reliable than Wi-Fi for race-day timing.",
      "Plan your IP address range in advance. Write down each device's IP so you can troubleshoot on-site.",
      "If using Wi-Fi, test the connection at the venue before race day to verify signal strength.",
    ],
    seeAlso: [
      { sectionKey: "sbc_identity", label: "Device Identity" },
      { sectionKey: "sbc_forwarder", label: "Forwarder Setup" },
    ],
  },
  sbc_forwarder: {
    title: "Forwarder Setup",
    overview:
      "Configure how the forwarder software on the SBC connects to the timing server and IPICO readers. These settings are applied during first boot.",
    fields: {
      server_base_url: {
        label: "Server Base URL",
        summary:
          "URL of the timing server this forwarder sends data to.",
        detailHtml:
          "The base URL of your timing server. Auto-filled with the current page's URL by default." +
          "<br><br>" +
          "Make sure this URL is reachable from the SBC's network at the venue.",
      },
      auth_token: {
        label: "Auth Token",
        summary:
          "Authentication token for the forwarder to connect to the server.",
        detailHtml:
          "Each forwarder needs a unique authentication token to connect to the server. Click <strong>Create Token</strong> to generate one automatically, or paste an existing token." +
          "<br><br>" +
          "The token is used to identify this forwarder and authorize its connection. Keep tokens secure — they grant access to send timing data to the server.",
      },
      reader_targets: {
        label: "Reader Targets",
        summary:
          "IP:port addresses of the IPICO readers this forwarder connects to, one per line.",
        detailHtml:
          "List the IP address and port of each IPICO reader this forwarder should connect to, one per line (e.g. <code>192.168.1.10:10000</code>)." +
          "<br><br>" +
          "Most IPICO Lite readers use port 10000. Elite readers typically use port 10100. Make sure each reader's IP matches its actual network address.",
        recommended: "Double-check reader IPs on-site before race day.",
      },
      status_bind: {
        label: "Status Bind Address",
        summary:
          "Address and port for the forwarder's local status web page.",
        detailHtml:
          "The IP:port where the forwarder exposes its local status page. Browse to this address from any device on the same network to check the forwarder's health." +
          "<br><br>" +
          "Use <code>0.0.0.0:80</code> to make the status page accessible from any device on the network.",
        default: "0.0.0.0:80",
      },
      display_name: {
        label: "Display Name",
        summary:
          "Optional friendly name shown in the dashboard instead of the forwarder ID.",
        detailHtml:
          "A human-readable label for this forwarder (e.g. 'Start Line', 'Finish Line'). When set, this name appears in the server dashboard and makes it easier to identify each forwarder at a glance.",
        default: "None (optional)",
        recommended: "Use descriptive names like 'Start Line' or 'Finish A' for easy identification.",
      },
      download_user_data: {
        label: "Download user-data",
        summary:
          "Downloads the setup file for flashing onto the SBC's SD card.",
        detailHtml:
          "Generates and downloads the provisioning file based on the current form values. This file is placed on the SD card's boot partition and runs during the SBC's first boot to install and configure the forwarder software.",
      },
      download_network_config: {
        label: "Download network-config",
        summary:
          "Downloads the network configuration file for flashing onto the SBC's SD card.",
        detailHtml:
          "Generates and downloads the network configuration file based on the current network settings. This file is placed on the SD card's boot partition and configures the SBC's network interfaces (Ethernet and optionally Wi-Fi) during first boot.",
      },
      save_next_device: {
        label: "Save & Next Device",
        summary:
          "Saves the current settings and auto-increments the hostname and IP for the next device.",
        detailHtml:
          "Saves the current form values to your browser's local storage, then auto-increments the hostname number and IP address for the next SBC. The auth token is cleared so you generate a fresh one for the next device." +
          "<br><br>" +
          "For example:" +
          "<ul>" +
          "<li><code>rt-fwd-01</code> becomes <code>rt-fwd-02</code></li>" +
          "<li><code>192.168.1.51/24</code> becomes <code>192.168.1.52/24</code></li>" +
          "</ul>",
      },
    },
    tips: [
      "Use <strong>Create Token</strong> for each device — every forwarder needs its own unique token.",
      "Use <strong>Save & Next Device</strong> when provisioning multiple SBCs to speed up the process.",
      "Test each SBC's network connectivity at the venue before race day.",
    ],
    seeAlso: [
      { sectionKey: "sbc_identity", label: "Device Identity" },
      { sectionKey: "sbc_network", label: "Network Configuration" },
      { sectionKey: "sbc_advanced", label: "Advanced" },
    ],
  },
  sbc_advanced: {
    title: "Advanced",
    overview:
      "Advanced provisioning settings. These usually don't need to be changed unless you're using a custom setup.",
    fields: {
      setup_script_url: {
        label: "Setup Script URL",
        summary:
          "URL of the setup script downloaded and run during the SBC's first boot.",
        detailHtml:
          "The URL of the shell script that runs during first-boot provisioning. This script installs the forwarder software and configures the system." +
          "<br><br>" +
          "Only change this if you're using a custom or forked version of the setup script.",
      },
    },
    tips: [
      "Leave the default setup script URL unless you have a specific reason to change it.",
    ],
    seeAlso: [{ sectionKey: "sbc_forwarder", label: "Forwarder Setup" }],
  },
  admin_streams: {
    title: "Streams",
    overview:
      "Manage active and offline streams. Deleting a stream permanently removes it along with all associated events, metrics, and receiver cursors.",
    fields: {
      delete_stream: {
        label: "Delete Stream",
        summary:
          "Permanently deletes this stream and all its associated data.",
        detailHtml:
          "<strong>This action is irreversible.</strong> Deleting a stream permanently removes:" +
          "<ul>" +
          "<li>All timing events for this stream</li>" +
          "<li>All metrics and read counts</li>" +
          "<li>All receiver cursor positions for this stream</li>" +
          "</ul>" +
          "The forwarder can re-create the stream by reconnecting, but all historical data will be gone.",
      },
      delete_all_streams: {
        label: "Delete All Streams",
        summary:
          "Permanently deletes every stream and all associated data.",
        detailHtml:
          "<strong>This action is irreversible.</strong> Removes every stream and all their events, metrics, and cursor positions. " +
          "This is equivalent to a full data reset for the timing system. Forwarders can re-create streams by reconnecting, but all historical data will be gone.",
      },
    },
    tips: [
      "Only delete streams after an event is fully complete and all data has been exported.",
      "If a stream is offline and you don't need its data, deleting it keeps the admin view clean.",
      "Forwarders will automatically re-create their streams on the next connection.",
    ],
    seeAlso: [
      { sectionKey: "admin_events", label: "Events" },
      { sectionKey: "admin_cursors", label: "Receiver Cursors" },
    ],
  },
  admin_events: {
    title: "Events",
    overview:
      "Delete stored timing events. You can delete all events, or narrow it to a specific stream or epoch.",
    fields: {
      event_stream_select: {
        label: "Stream",
        summary:
          "Scopes the delete to a single stream, or all streams if 'All Streams' is selected.",
        detailHtml:
          "Select a specific stream to delete events from, or leave on <strong>All Streams</strong> to clear events across every stream." +
          "<br><br>" +
          "Selecting a stream also shows the epoch selector, allowing you to further narrow the scope.",
      },
      event_epoch_select: {
        label: "Epoch",
        summary:
          "Further scopes the delete to a specific epoch within the selected stream.",
        detailHtml:
          "After selecting a stream, you can choose a specific epoch to clear. Each epoch represents a time segment (e.g. a race or wave). The event count and date are shown for each epoch." +
          "<br><br>" +
          "Select <strong>All Epochs</strong> to clear all events for the selected stream.",
      },
      clear_events: {
        label: "Clear Events",
        summary:
          "Permanently deletes timing events matching the current stream and epoch selection.",
        detailHtml:
          "<strong>This action is irreversible.</strong> Deletes all timing events that match your current selection:" +
          "<ul>" +
          "<li><strong>All Streams</strong>: Deletes every event in the system</li>" +
          "<li><strong>Specific stream, All Epochs</strong>: Deletes all events for that stream</li>" +
          "<li><strong>Specific stream + epoch</strong>: Deletes events for that stream/epoch only</li>" +
          "</ul>" +
          "Events are the raw chip reads — once deleted, they cannot be recovered.",
      },
    },
    tips: [
      "Use epoch-level deletion to clean up test data from a specific period without affecting real race data.",
      "Export or back up any needed timing data before clearing events.",
      "Clearing events does not affect receiver cursors — receivers may still have copies of the data.",
    ],
    seeAlso: [
      { sectionKey: "admin_streams", label: "Streams" },
      { sectionKey: "admin_cursors", label: "Receiver Cursors" },
    ],
  },
  admin_tokens: {
    title: "Device Tokens",
    overview:
      "Create and manage authentication tokens for forwarders and receivers. Each device needs a unique token to connect. Revoking a token prevents the device from reconnecting.",
    fields: {
      create_token: {
        label: "Create Token",
        summary:
          "Creates a new authentication token for a forwarder or receiver device.",
        detailHtml:
          "Enter a <strong>Device ID</strong> (a name to identify the device), select the device type (forwarder or receiver), and optionally provide a custom token value." +
          "<br><br>" +
          "If you leave the token field blank, a secure token is generated automatically. <strong>Copy or download the token immediately</strong> — it cannot be retrieved later.",
      },
      revoke_token: {
        label: "Revoke Token",
        summary:
          "Prevents the device from reconnecting. Existing connections may stay alive temporarily.",
        detailHtml:
          "Revoking a token marks it as invalid. The device will not be able to establish new connections. An existing connection may remain active until it disconnects naturally." +
          "<br><br>" +
          "Revoked tokens remain visible in the list (marked as 'Revoked') for auditing. Use <strong>Delete All Tokens</strong> to fully remove them.",
      },
      delete_all_tokens: {
        label: "Delete All Tokens",
        summary:
          "Permanently removes all tokens — both active and revoked.",
        detailHtml:
          "<strong>This action is irreversible.</strong> Removes every device token from the system. All devices will need new tokens to reconnect." +
          "<br><br>" +
          "Existing connections may stay alive temporarily, but once disconnected, the devices cannot reconnect without new tokens.",
      },
    },
    tips: [
      "Create tokens during SBC setup — each forwarder device needs its own unique token.",
      "Revoke a token instead of deleting it if you might need to audit which devices were authorized.",
      "After rotating tokens, make sure to update the forwarder configurations with the new tokens.",
    ],
    seeAlso: [
      { sectionKey: "sbc_forwarder", label: "Forwarder Setup" },
    ],
  },
  admin_cursors: {
    title: "Receiver Cursors",
    overview:
      "Manage receiver sync positions. A cursor tracks where each receiver left off in each stream, so it only receives new reads on reconnection. Clearing a cursor forces a full re-sync from the beginning.",
    fields: {
      delete_cursor: {
        label: "Delete Cursor",
        summary:
          "Clears this receiver's sync position for this specific stream.",
        detailHtml:
          "Removes the cursor for one receiver on one stream. The next time this receiver connects, it will re-sync all events for this stream from the beginning." +
          "<br><br>" +
          "Use this if a receiver missed data and needs to re-download events for a specific stream.",
      },
      delete_receiver_cursors: {
        label: "Delete All for Receiver",
        summary:
          "Clears all cursor positions for this receiver across all streams.",
        detailHtml:
          "Removes all cursors for one receiver. The next time it connects, it will re-sync all events for every stream from the beginning." +
          "<br><br>" +
          "Use this if a receiver's local data is corrupted or out of sync.",
      },
      clear_all_cursors: {
        label: "Clear All Cursors",
        summary:
          "Clears every receiver's sync position. All receivers will re-sync from the beginning.",
        detailHtml:
          "<strong>Use with caution.</strong> Removes all cursor positions for every receiver. Every receiver will re-download all events from scratch on their next connection." +
          "<br><br>" +
          "This is useful after a major data reset or if you need all receivers to start fresh.",
      },
    },
    tips: [
      "Clear a single receiver's cursors if it missed data. Avoid clearing all cursors unless necessary — it causes every receiver to re-download everything.",
      "Re-syncing is safe but may take time if there are many events. Plan cursor resets during downtime, not during active timing.",
    ],
    seeAlso: [
      { sectionKey: "admin_streams", label: "Streams" },
      { sectionKey: "admin_events", label: "Events" },
    ],
  },
  admin_races: {
    title: "Races",
    overview:
      "Bulk delete all races and associated data. This removes all races, participants, chip mappings, and forwarder-race associations from the system.",
    fields: {
      delete_all_races: {
        label: "Delete All Races",
        summary:
          "Permanently deletes every race and all associated participant and chip data.",
        detailHtml:
          "<strong>This action is irreversible.</strong> Removes all races along with:" +
          "<ul>" +
          "<li>All participant records</li>" +
          "<li>All chip-to-bib mappings</li>" +
          "<li>All forwarder-race associations</li>" +
          "</ul>" +
          "Timing data (reads/events) is not affected — only race metadata is deleted. The announcer and dashboard will stop resolving chip reads to names until new race data is uploaded.",
      },
    },
    tips: [
      "Only delete all races after the event is complete and results have been finalized.",
      "Timing data is preserved — you can re-upload race data later and reads will resolve again.",
    ],
    seeAlso: [
      { sectionKey: "races", label: "Races" },
      { sectionKey: "admin_events", label: "Events" },
    ],
  },
} as const satisfies HelpContext;
