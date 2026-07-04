import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ForwarderReaderCountsUpdate,
  ReceiverMode,
  StatusResponse,
  StreamMetrics,
  StreamsResponse,
  UpsStatus,
} from "./api";

// Payload types matching the Rust ReceiverUiEvent serde output.
// Each variant serializes with #[serde(tag = "type", rename_all = "snake_case")].
type StatusChangedPayload = {
  connection_state: StatusResponse["connection_state"];
  local_ok?: boolean;
  streams_count: number;
  receiver_id?: string;
};
type StatusChangedUpdate = Omit<StatusResponse, "server">;
type StreamsSnapshotPayload = {
  streams: StreamsResponse["streams"];
  degraded: boolean;
  upstream_error?: string | null;
};
type LogEntryPayload = { entry: string };
type ForwarderReaderCountsUpdatedPayload = ForwarderReaderCountsUpdate;
type ModeChangedPayload = { mode: ReceiverMode };
type LastReadPayload = {
  forwarder_id: string;
  reader_ip: string;
  chip_id: string;
  timestamp: string;
  bib?: string | null;
  name?: string | null;
};

export type ForwarderUpsUpdatedPayload = {
  forwarder_id: string;
  available: boolean;
  status: UpsStatus | null;
};

// One stream's coalesced update from the backend's 4-10 Hz delta emitter.
export type StreamDeltaPayload = {
  forwarder_endpoint_id: string;
  stream_id: string;
  forwarder_id: string;
  reader_ip: string;
  reads_total: number;
  reads_epoch: number;
  metrics: StreamMetrics;
  last_read?: LastReadPayload | null;
};
type StreamDeltasPayload = { updates?: StreamDeltaPayload[] };

export type SseCallbacks = {
  onStatusChanged: (status: StatusChangedUpdate) => void;
  onStreamsSnapshot: (streams: StreamsResponse) => void;
  onLogEntry: (entry: string) => void;
  onResync: () => void;
  onConnectionsChanged: () => void;
  onConnectionChange: (connected: boolean) => void;
  onForwarderReaderCountsUpdated: (update: ForwarderReaderCountsUpdate) => void;
  onModeChanged: (mode: ReceiverMode) => void;
  onStreamDeltas: (updates: StreamDeltaPayload[]) => void;
  onForwarderUpsUpdated?: (payload: ForwarderUpsUpdatedPayload) => void;
};

let unlistenFns: UnlistenFn[] = [];

export async function initSSE(callbacks: SseCallbacks): Promise<void> {
  if (unlistenFns.length > 0) return;

  // Tauri events are always connected (in-process)
  callbacks.onConnectionChange(true);

  unlistenFns = await Promise.all([
    listen<StatusChangedPayload>("status_changed", (event) => {
      callbacks.onStatusChanged({
        connection_state: event.payload.connection_state,
        local_ok: event.payload.local_ok ?? true,
        streams_count: event.payload.streams_count,
        receiver_id: event.payload.receiver_id ?? "",
      });
    }),
    listen<StreamsSnapshotPayload>("streams_snapshot", (event) => {
      callbacks.onStreamsSnapshot({
        streams: event.payload.streams,
        degraded: event.payload.degraded,
        upstream_error: event.payload.upstream_error ?? null,
      });
    }),
    listen<LogEntryPayload>("log_entry", (event) => {
      callbacks.onLogEntry(event.payload.entry);
    }),
    listen("resync", () => {
      callbacks.onResync();
    }),
    listen("connections_changed", () => {
      callbacks.onConnectionsChanged();
    }),
    listen<ForwarderReaderCountsUpdatedPayload>(
      "forwarder_reader_counts_updated",
      (event) => {
        callbacks.onForwarderReaderCountsUpdated({
          forwarder_id: event.payload.forwarder_id,
          stream_id: event.payload.stream_id,
          reads_session: event.payload.reads_session,
          reads_total: event.payload.reads_total,
          last_read_unix_ms: event.payload.last_read_unix_ms ?? null,
          last_seen_secs: event.payload.last_seen_secs ?? null,
        });
      },
    ),
    listen<ModeChangedPayload>("mode_changed", (event) => {
      callbacks.onModeChanged(event.payload.mode);
    }),
    listen<StreamDeltasPayload>("stream_deltas", (event) => {
      callbacks.onStreamDeltas(event.payload.updates ?? []);
    }),
    listen<ForwarderUpsUpdatedPayload>("forwarder_ups_updated", (event) => {
      callbacks.onForwarderUpsUpdated?.(event.payload);
    }),
  ]);
}

export function destroySSE(): void {
  for (const unlisten of unlistenFns) {
    unlisten();
  }
  unlistenFns = [];
}
