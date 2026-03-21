import { listen, type UnlistenFn } from "@tauri-apps/api/event";
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

let unlistenFns: UnlistenFn[] = [];

export async function initSSE(callbacks: SseCallbacks): Promise<void> {
  if (unlistenFns.length > 0) return;

  // Tauri events are always connected (in-process)
  callbacks.onConnectionChange(true);

  unlistenFns = await Promise.all([
    listen<any>("status_changed", (event) => {
      callbacks.onStatusChanged({
        connection_state: event.payload.connection_state,
        local_ok: event.payload.local_ok ?? true,
        streams_count: event.payload.streams_count,
        receiver_id: event.payload.receiver_id ?? "",
      });
    }),
    listen<any>("streams_snapshot", (event) => {
      callbacks.onStreamsSnapshot({
        streams: event.payload.streams,
        degraded: event.payload.degraded,
        upstream_error: event.payload.upstream_error ?? null,
      });
    }),
    listen<any>("log_entry", (event) => {
      callbacks.onLogEntry(event.payload.entry);
    }),
    listen("resync", () => {
      callbacks.onResync();
    }),
    listen<any>("stream_counts_updated", (event) => {
      callbacks.onStreamCountsUpdated(event.payload.updates ?? []);
    }),
    listen<any>("mode_changed", (event) => {
      callbacks.onModeChanged(event.payload.mode);
    }),
    listen<any>("last_read", (event) => {
      callbacks.onLastRead({
        forwarder_id: event.payload.forwarder_id,
        reader_ip: event.payload.reader_ip,
        chip_id: event.payload.chip_id,
        timestamp: event.payload.timestamp,
        bib: event.payload.bib ?? null,
        name: event.payload.name ?? null,
      });
    }),
  ]);
}

export function destroySSE(): void {
  for (const unlisten of unlistenFns) {
    unlisten();
  }
  unlistenFns = [];
}
