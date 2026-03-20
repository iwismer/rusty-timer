import { createSSE, type SseHandle } from "@rusty-timer/shared-ui/lib/sse";
import type {
  LastRead,
  ReceiverMode,
  StatusResponse,
  StreamCountUpdate,
  StreamsResponse,
} from "./api";

export type SseCallbacks = {
  onStatusChanged: (status: StatusResponse) => void;
  onStreamsSnapshot: (streams: StreamsResponse) => void;
  onLogEntry: (entry: string) => void;
  onResync: () => void;
  onConnectionChange: (connected: boolean) => void;
  onStreamCountsUpdated: (updates: StreamCountUpdate[]) => void;
  onModeChanged: (mode: ReceiverMode) => void;
  onLastRead: (read: LastRead) => void;
};

let handle: SseHandle | null = null;

export function initSSE(callbacks: SseCallbacks): void {
  if (handle) return;

  handle = createSSE(
    "/api/v1/events",
    {
      status_changed: (data: any) => {
        callbacks.onStatusChanged({
          connection_state: data.connection_state,
          local_ok: data.local_ok ?? true,
          streams_count: data.streams_count,
          receiver_id: data.receiver_id ?? "",
        });
      },
      streams_snapshot: (data: any) => {
        callbacks.onStreamsSnapshot({
          streams: data.streams,
          degraded: data.degraded,
          upstream_error: data.upstream_error ?? null,
        });
      },
      log_entry: (data: any) => {
        callbacks.onLogEntry(data.entry);
      },
      resync: () => {
        callbacks.onResync();
      },
      stream_counts_updated: (data: any) => {
        callbacks.onStreamCountsUpdated(data.updates ?? []);
      },
      mode_changed: (data: any) => {
        callbacks.onModeChanged(data.mode);
      },
      last_read: (data: any) => {
        callbacks.onLastRead({
          forwarder_id: data.forwarder_id,
          reader_ip: data.reader_ip,
          chip_id: data.chip_id,
          timestamp: data.timestamp,
          bib: data.bib ?? null,
          name: data.name ?? null,
        });
      },
    },
    (connected) => {
      callbacks.onConnectionChange(connected);
      if (connected) callbacks.onResync();
    },
  );
}

export function destroySSE(): void {
  if (handle) {
    handle.destroy();
    handle = null;
  }
}
