import type { HelpContext } from "./help-types";

export const RECEIVER_ADMIN_HELP = {
  cursor_reset: {
    title: "Cursor Reset",
    overview: "Reset resume cursors to replay data from the beginning. Cursors track the receiver's position in each stream so it can resume where it left off after a disconnect.",
    fields: {
      stream_cursor: {
        label: "Stream Cursor",
        summary: "Current read position in the stream (epoch + sequence number).",
        detail: "Each stream has a cursor that tracks the last successfully received read. When the receiver reconnects, it resumes from this position. Resetting a cursor causes the stream to replay all data from the beginning on the next connection. This is safe to do: reads are idempotent and your timing software should handle duplicates.",
      },
    },
    tips: [
      "Reset a cursor when you need to replay all historical data for a specific stream.",
      "Resetting a cursor does NOT affect the server. It only changes where the receiver starts reading on next connect.",
      "After resetting, the receiver will re-deliver all reads from the start. Your timing software may see duplicate reads.",
      "Try cursor reset before more drastic actions like purge subscriptions or factory reset.",
    ],
    seeAlso: [
      { sectionKey: "epoch_overrides", label: "Earliest-Epoch Overrides" },
      { sectionKey: "purge_subscriptions", label: "Purge Subscriptions" },
    ],
  },
  epoch_overrides: {
    title: "Earliest-Epoch Overrides",
    overview: "Clear earliest-epoch overrides to receive all available data. Epoch overrides control the starting point for data delivery per stream.",
    fields: {
      epoch_override: {
        label: "Epoch Override",
        summary: "The earliest epoch the stream will deliver data from.",
        detail: "Earliest-epoch overrides filter out data older than the specified epoch. Clearing an override causes the stream to deliver data from all available epochs instead of just recent ones. This is useful when you need access to historical data that was previously filtered out.",
      },
    },
    tips: [
      "Clear epoch overrides when you need to access historical data that was previously filtered.",
      "This only affects the receiver's filtering. The server always has all data available.",
      "After clearing, the receiver may re-deliver older reads. Combine with a cursor reset if needed.",
    ],
    seeAlso: [
      { sectionKey: "cursor_reset", label: "Cursor Reset" },
    ],
  },
  port_overrides: {
    title: "Local Port Overrides",
    overview: "Customize the local TCP port used to forward reads from each stream to your timing software.",
    fields: {
      port_override: {
        label: "Port Override",
        summary: "Custom local port for forwarding reads from this stream.",
        detail: "By default, each stream's reads are forwarded to a local port calculated as 10000 + the last octet of the reader's IP address (e.g. reader at 192.168.0.50 uses port 10050). Set a port override to use a different port. Leave empty to use the default. Common timing software ports: RunScore typically uses 10000-10010, many IPICO setups use the reader's native port mapping.",
        default: "10000 + last IP octet",
        range: "1-65535",
        recommended: "Use the default unless your timing software requires a specific port.",
      },
    },
    tips: [
      "Only set port overrides if your timing software expects data on a specific port.",
      "Port changes take effect immediately. Make sure your timing software is listening on the new port.",
      "Clear a port override (leave empty) to revert to the default calculation.",
    ],
    seeAlso: [
      { sectionKey: "cursor_reset", label: "Cursor Reset" },
    ],
  },
  purge_subscriptions: {
    title: "Purge Subscriptions",
    overview: "Remove all stream subscriptions. The receiver will stop receiving data from all streams until new subscriptions are created.",
    fields: {},
    tips: [
      "Purging subscriptions stops all data delivery. The receiver will have zero active streams.",
      "Stream data is NOT deleted from the server. You can re-subscribe to streams after purging.",
      "In Live mode, the receiver will automatically re-subscribe to available streams after purging.",
      "Try this when streams are in a bad state and you want a clean start without a full factory reset.",
      "Cursor positions and epoch overrides are also cleared when subscriptions are purged.",
    ],
    seeAlso: [
      { sectionKey: "cursor_reset", label: "Cursor Reset" },
      { sectionKey: "factory_reset", label: "Factory Reset" },
    ],
  },
  reset_profile: {
    title: "Reset Profile",
    overview: "Clear the receiver's connection profile (server URL, token, and receiver ID) back to defaults. Subscriptions and cursors are preserved.",
    fields: {},
    tips: [
      "Use this when you need to point the receiver at a different server.",
      "Subscriptions and cursor positions are preserved. Only connection settings are cleared.",
      "After resetting, you must reconfigure the Server URL, Token, and Receiver ID before connecting.",
      "The receiver will disconnect automatically when the profile is reset.",
    ],
    seeAlso: [
      { sectionKey: "factory_reset", label: "Factory Reset" },
    ],
  },
  factory_reset: {
    title: "Factory Reset",
    overview: "Erase ALL local data and return the receiver to a fresh state. This is irreversible.",
    fields: {},
    tips: [
      "BEFORE factory reset, try these less destructive alternatives first:",
      "1. Cursor Reset: if you just need to replay data from the beginning.",
      "2. Purge Subscriptions: if streams are in a bad state and you want a clean start.",
      "3. Reset Profile: if you just need to change the server connection.",
      "Factory reset deletes: profile (server URL, token, ID), all subscriptions, all cursors, all epoch overrides, and all port overrides.",
      "After factory reset, the receiver must be fully reconfigured from scratch.",
      "This action CANNOT be undone. All local state is permanently deleted.",
      "The receiver will disconnect immediately and return to the initial setup state.",
    ],
    seeAlso: [
      { sectionKey: "reset_profile", label: "Reset Profile" },
      { sectionKey: "purge_subscriptions", label: "Purge Subscriptions" },
      { sectionKey: "cursor_reset", label: "Cursor Reset" },
    ],
  },
} as const satisfies HelpContext;
