import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ForwarderMetricsUpdate,
  LastRead,
  ReceiverMode,
  StatusResponse,
  StreamCountUpdate,
  StreamMetrics,
  StreamsResponse,
} from "./api";

// Payload types matching the Rust ReceiverUiEvent serde output.
// Each variant serializes with #[serde(tag = "type", rename_all = "snake_case")].
type StatusChangedPayload = {
  connection_state: StatusResponse["connection_state"];
  local_ok?: boolean;
  streams_count: number;
  receiver_id?: string;
};
type StreamsSnapshotPayload = {
  streams: StreamsResponse["streams"];
  degraded: boolean;
  upstream_error?: string | null;
};
type LogEntryPayload = { entry: string };
type StreamCountsUpdatedPayload = { updates?: StreamCountUpdate[] };
type ForwarderMetricsUpdatedPayload = ForwarderMetricsUpdate;
type ModeChangedPayload = { mode: ReceiverMode };
type LastReadPayload = {
  forwarder_id: string;
  reader_ip: string;
  chip_id: string;
  timestamp: string;
  bib?: string | null;
  name?: string | null;
};

export type ReaderInfoUpdatedPayload = {
  stream_id: string;
  reader_ip: string;
  state: string;
  reader_info: unknown;
};

export type ReaderDownloadProgressPayload = {
  stream_id: string;
  reader_ip: string;
  state: string;
  reads_received: number;
  progress: number;
  total: number;
  error?: string | null;
};

export type SseCallbacks = {
  onStatusChanged: (status: StatusResponse) => void;
  onStreamsSnapshot: (streams: StreamsResponse) => void;
  onLogEntry: (entry: string) => void;
  onResync: () => void;
  onConnectionChange: (connected: boolean) => void;
  onStreamCountsUpdated: (updates: StreamCountUpdate[]) => void;
  onForwarderMetricsUpdated: (update: ForwarderMetricsUpdate) => void;
  onModeChanged: (mode: ReceiverMode) => void;
  onLastRead: (read: LastRead) => void;
  onStreamMetricsUpdated: (metrics: StreamMetrics) => void;
  onReaderInfoUpdated?: (payload: ReaderInfoUpdatedPayload) => void;
  onReaderDownloadProgress?: (payload: ReaderDownloadProgressPayload) => void;
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
    listen<StreamCountsUpdatedPayload>("stream_counts_updated", (event) => {
      callbacks.onStreamCountsUpdated(event.payload.updates ?? []);
    }),
    listen<ForwarderMetricsUpdatedPayload>(
      "forwarder_metrics_updated",
      (event) => {
        callbacks.onForwarderMetricsUpdated(event.payload);
      },
    ),
    listen<ModeChangedPayload>("mode_changed", (event) => {
      callbacks.onModeChanged(event.payload.mode);
    }),
    listen<LastReadPayload>("last_read", (event) => {
      callbacks.onLastRead({
        forwarder_id: event.payload.forwarder_id,
        reader_ip: event.payload.reader_ip,
        chip_id: event.payload.chip_id,
        timestamp: event.payload.timestamp,
        bib: event.payload.bib ?? null,
        name: event.payload.name ?? null,
      });
    }),
    listen<StreamMetrics>("stream_metrics_updated", (event) => {
      callbacks.onStreamMetricsUpdated({
        forwarder_id: event.payload.forwarder_id,
        reader_ip: event.payload.reader_ip,
        raw_count: event.payload.raw_count,
        dedup_count: event.payload.dedup_count,
        retransmit_count: event.payload.retransmit_count,
        lag_ms: event.payload.lag_ms ?? null,
        epoch_raw_count: event.payload.epoch_raw_count,
        epoch_dedup_count: event.payload.epoch_dedup_count,
        epoch_retransmit_count: event.payload.epoch_retransmit_count,
        unique_chips: event.payload.unique_chips,
        epoch_last_received_at: event.payload.epoch_last_received_at ?? null,
        epoch_lag_ms: event.payload.epoch_lag_ms ?? null,
      });
    }),
    listen<ReaderInfoUpdatedPayload>("reader_info_updated", (event) => {
      callbacks.onReaderInfoUpdated?.(event.payload);
    }),
    listen<ReaderDownloadProgressPayload>(
      "reader_download_progress",
      (event) => {
        callbacks.onReaderDownloadProgress?.(event.payload);
      },
    ),
  ]);
}

export function destroySSE(): void {
  for (const unlisten of unlistenFns) {
    unlisten();
  }
  unlistenFns = [];
}
