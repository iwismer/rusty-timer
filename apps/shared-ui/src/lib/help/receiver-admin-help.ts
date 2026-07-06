import type { HelpContext } from './help-types';

export const RECEIVER_ADMIN_HELP = {
  cursor_reset: {
    title: 'Cursor Reset',
    overview:
      "Reset resume cursors to replay data from the beginning. Cursors track the receiver's position in each stream so it can resume where it left off after a disconnect.",
    fields: {
      stream_cursor: {
        label: 'Stream Cursor',
        summary: 'Current read position in the stream.',
        detailHtml:
          'Each stream has a cursor that tracks the last successfully received read. When the receiver reconnects, it resumes from this position.<br><br>Resetting a cursor causes the stream to replay all data from the beginning on the next connection. This is safe to do — your timing software should handle duplicate reads.',
      },
      reset_cursor: {
        label: 'Reset Cursor',
        summary: 'Reset one stream so it replays from the beginning on next connect.',
        detailHtml:
          "Use this when one stream needs to re-send all stored reads. The reset changes only the receiver's resume position; it does not delete data from the forwarder or the receiver.",
      },
      reset_all_cursors: {
        label: 'Reset All Cursors',
        summary: 'Reset every stream cursor at once.',
        detailHtml:
          'Use this when your timing software needs a full re-delivery from every subscribed stream. All streams replay from their beginning on the next connection, so duplicates may appear in downstream software.',
      },
      reset_stream_data: {
        label: 'Reset Stream Data',
        summary:
          "Delete one stream's locally stored reads so it re-fetches from scratch.",
        detailHtml:
          "Deletes the stream's locally received reads, gap markers, and cursor, then reconnects so the stream re-fetches from the forwarder. The subscription and any <strong>From epoch</strong> override are preserved.<br><br>Combined with a From-epoch override, this is the epoch-replay recovery recipe: set the override to the race's epoch, reset the stream's local data, and reconnect your timing software to the local port — it receives only the chosen epoch onward.<br><br>Local consumers connected to this stream's port will see the data replayed as it re-arrives.",
      },
    },
    tips: [
      'Reset a cursor when you need to replay all historical data for a specific stream.',
      "Resetting a cursor only changes where the receiver starts reading on the next connection. It does not affect the forwarder's journal.",
      'After resetting, the receiver will re-deliver all reads from the start. Your timing software may see duplicate reads.',
      'Try cursor reset before more drastic actions like purge subscriptions or factory reset.',
    ],
    seeAlso: [
      { sectionKey: 'epoch_overrides', label: 'Earliest-Epoch Overrides' },
      { sectionKey: 'purge_subscriptions', label: 'Purge Subscriptions' },
    ],
  },
  epoch_overrides: {
    title: 'Earliest-Epoch Overrides',
    overview:
      'Clear earliest-epoch overrides to remove future fetch floors. Epoch overrides control the starting point used the next time each stream subscribes; clearing them does not back-fill reads that were already skipped.',
    fields: {
      epoch_override: {
        label: 'Epoch Override',
        summary: 'The earliest epoch the stream will deliver data from.',
        detailHtml:
          'Earliest-epoch overrides tell the receiver to ask for reads from the specified epoch onward when the stream subscribes. Clearing an override removes that future fetch floor, but it does not delete local data, move the stream cursor backward, or back-fill reads that this receiver already skipped.<br><br>To re-fetch older retained reads, clear or change the override first, then use <strong>Reset Stream Data</strong> for the stream (or reset its cursor when you only need a replay) before reconnecting your timing software.',
      },
      reset_epoch_override: {
        label: 'Reset Epoch',
        summary: "Clear one stream's earliest-epoch override.",
        detailHtml:
          "Clears the selected stream's earliest-epoch override so future subscriptions no longer ask the forwarder to skip earlier epochs. This affects the future fetch floor only; it does not delete data, reset the cursor, or back-fill reads already skipped. Use Reset Stream Data or a cursor reset when you need to re-fetch older retained reads.",
      },
      reset_all_epoch_overrides: {
        label: 'Reset All Epoch Overrides',
        summary: 'Clear earliest-epoch overrides for every stream.',
        detailHtml:
          'Removes every earliest-epoch override. Afterward, future stream subscriptions no longer ask forwarders to skip earlier epochs, but existing cursors are not moved backward and skipped reads are not back-filled automatically. Use Reset Stream Data or cursor resets for streams that must re-fetch older retained reads.',
      },
    },
    tips: [
      'Clear epoch overrides before resetting stream data when you need to re-fetch older retained reads.',
      "This only affects the receiver's future fetch floor. The forwarder's journal still determines which epochs are retained.",
      'Clearing an override alone does not move a stream cursor backward or back-fill reads already skipped.',
    ],
    seeAlso: [{ sectionKey: 'cursor_reset', label: 'Cursor Reset' }],
  },
  port_overrides: {
    title: 'Local Port Overrides',
    overview:
      'Customize the local port used to forward reads from each stream to your timing software.',
    fields: {
      port_override: {
        label: 'Port Override',
        summary: 'Custom local port for forwarding reads from this stream.',
        detailHtml:
          "For standard reader ports, each stream's reads are forwarded to a default local port calculated as <strong>10000 + the last octet of the reader's IP address</strong> (e.g. reader at 192.168.0.50 uses port 10050). Readers using non-standard source ports get a deterministic fallback port. Set a port override to use a different port. Leave empty to use the default.",
        default: 'Usually 10000 + last IP octet',
        range: '1-65535',
        recommended: 'Use the default unless your timing software requires a specific port.',
      },
    },
    tips: [
      'Only set port overrides if your timing software expects data on a specific port.',
      'Port changes take effect immediately. Make sure your timing software is listening on the new port.',
      'Clear a port override (leave empty) to revert to the default calculation.',
    ],
    seeAlso: [{ sectionKey: 'cursor_reset', label: 'Cursor Reset' }],
  },
  purge_subscriptions: {
    title: 'Purge Subscriptions',
    overview:
      'Remove all stream subscriptions. The receiver will stop receiving data from all streams until new subscriptions are created.',
    fields: {
      purge_all_subscriptions: {
        label: 'Purge All Subscriptions',
        summary: 'Remove every local stream subscription.',
        detailHtml:
          'Stops local delivery for all streams and clears their subscription records. Received stream data is not deleted. In Live mode, available streams may be automatically subscribed again after the receiver refreshes.',
      },
    },
    tips: [
      'Purging subscriptions stops all data delivery. The receiver will have zero active streams.',
      'Received stream data is NOT deleted. You can re-subscribe to streams after purging.',
      'In Live mode, the receiver will automatically re-subscribe to available streams after purging.',
      'Try this when streams are in a bad state and you want a clean start without a full factory reset.',
      'Purging deletes subscription records only. Cursor positions, epoch overrides, and locally received reads are preserved; use Reset Stream Data, Clear Data, or Factory Reset for broader cleanup.',
    ],
    seeAlso: [
      { sectionKey: 'cursor_reset', label: 'Cursor Reset' },
      { sectionKey: 'clear_data', label: 'Clear Data' },
      { sectionKey: 'factory_reset', label: 'Factory Reset' },
    ],
  },
  reset_profile: {
    title: 'Reset Profile',
    overview:
      "Clear the receiver's connection profile (server URL, token, and receiver ID) back to defaults. Subscriptions and cursors are preserved.",
    fields: {
      reset_profile_action: {
        label: 'Reset Profile to Defaults',
        summary: 'Clear server URL, token, and receiver ID.',
        detailHtml:
          'Use this when you need to point the receiver at a different server or regenerate its identity. Stream subscriptions and cursor positions are preserved, but the receiver cannot reconnect until a new profile is saved.',
      },
    },
    tips: [
      'Use this when you need to point the receiver at a different server.',
      'Subscriptions and cursor positions are preserved. Only connection settings are cleared.',
      'After resetting, you must reconfigure the server URL, token, and receiver ID before connecting.',
      'The receiver will disconnect automatically when the profile is reset.',
    ],
    seeAlso: [{ sectionKey: 'factory_reset', label: 'Factory Reset' }],
  },
  clear_data: {
    title: 'Clear Data',
    overview:
      'Delete local stream data and state while keeping the server connection profile. Use this between events to start fresh without reconfiguring the receiver.',
    fields: {
      clear_data_action: {
        label: 'Clear Data',
        summary: 'Delete local reads and stream state, but keep receiver configuration.',
        detailHtml:
          'Deletes all received reads, subscriptions, cursors, epoch overrides, and gap markers. It also resets receiver mode, disables DBF output and announcer publishing, and clears per-stream announcer publish selections. The server URL, token, and receiver ID are preserved.',
      },
    },
    tips: [
      'Clear Data deletes all locally received reads. Data still retained by forwarders can be re-delivered by re-subscribing or replaying.',
      'The server URL, token, and receiver ID are preserved — the receiver stays configured and can reconnect immediately.',
      'Use Factory Reset instead if you also need to clear the connection profile.',
    ],
    seeAlso: [
      { sectionKey: 'purge_subscriptions', label: 'Purge Subscriptions' },
      { sectionKey: 'factory_reset', label: 'Factory Reset' },
    ],
  },
  factory_reset: {
    title: 'Factory Reset',
    overview:
      'Erase the receiver profile and local event/stream/operator state. This is irreversible.',
    fields: {
      factory_reset_action: {
        label: 'Factory Reset',
        summary: 'Erase all receiver-local data and configuration.',
        detailHtml:
          'Deletes the receiver profile, subscriptions, cursors, received reads, epoch overrides, port overrides, DBF settings, announcer settings, and local participant/chip data. This resets the receiver for normal operator use and cannot be undone.',
      },
    },
    tips: [
      'Before factory reset, try these less destructive alternatives first:<ul><li><strong>Cursor Reset</strong> — if you just need to replay data from the beginning.</li><li><strong>Purge Subscriptions</strong> — if streams are in a bad state and you want a clean start.</li><li><strong>Clear Data</strong> — if you need to delete event data but keep the server profile.</li><li><strong>Reset Profile</strong> — if you just need to change the server connection.</li></ul>',
      'Factory reset deletes the operator-facing receiver state: profile (server URL, token, ID), received reads, subscriptions, cursor positions, epoch overrides, port overrides, DBF settings, announcer settings, and imported participant/chip data.',
      'After factory reset, the receiver must be fully reconfigured from scratch.',
      'This action <strong>cannot be undone</strong>. Operator-facing local state is permanently deleted.',
      'The receiver will disconnect immediately and return to the initial setup state.',
    ],
    seeAlso: [
      { sectionKey: 'reset_profile', label: 'Reset Profile' },
      { sectionKey: 'clear_data', label: 'Clear Data' },
      { sectionKey: 'purge_subscriptions', label: 'Purge Subscriptions' },
      { sectionKey: 'cursor_reset', label: 'Cursor Reset' },
    ],
  },
} as const satisfies HelpContext;
