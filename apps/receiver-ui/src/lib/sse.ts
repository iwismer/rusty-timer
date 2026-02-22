import { createSSE, type SseHandle } from "@rusty-timer/shared-ui/lib/sse";
import type {
  StatusResponse,
  StreamsResponse,
  UpdateStatusResponse,
} from "./api";

export type SseCallbacks = {
  onStatusChanged: (status: StatusResponse) => void;
  onStreamsSnapshot: (streams: StreamsResponse) => void;
  onLogEntry: (entry: string) => void;
  onResync: () => void;
  onConnectionChange: (connected: boolean) => void;
  onUpdateStatusChanged: (status: UpdateStatusResponse) => void;
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
          local_ok: true,
          streams_count: data.streams_count,
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
      update_status_changed: (data: any) => {
        callbacks.onUpdateStatusChanged(data.status);
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
