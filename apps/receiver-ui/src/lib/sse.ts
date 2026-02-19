import type { StatusResponse, StreamsResponse, LogsResponse } from "./api";

const SSE_BASE = "";

export type SseCallbacks = {
  onStatusChanged: (status: StatusResponse) => void;
  onStreamsSnapshot: (streams: StreamsResponse) => void;
  onLogEntry: (entry: string) => void;
  onResync: () => void;
  onConnectionChange: (connected: boolean) => void;
  onUpdateAvailable: (version: string, currentVersion: string) => void;
};

let eventSource: EventSource | null = null;

export function initSSE(callbacks: SseCallbacks): void {
  if (eventSource) return;

  eventSource = new EventSource(`${SSE_BASE}/api/v1/events`);

  eventSource.addEventListener("status_changed", (e: MessageEvent) => {
    const data = JSON.parse(e.data);
    callbacks.onStatusChanged({
      connection_state: data.connection_state,
      local_ok: true,
      streams_count: data.streams_count,
    });
  });

  eventSource.addEventListener("streams_snapshot", (e: MessageEvent) => {
    const data = JSON.parse(e.data);
    callbacks.onStreamsSnapshot({
      streams: data.streams,
      degraded: data.degraded,
      upstream_error: data.upstream_error ?? null,
    });
  });

  eventSource.addEventListener("log_entry", (e: MessageEvent) => {
    const data = JSON.parse(e.data);
    callbacks.onLogEntry(data.entry);
  });

  eventSource.addEventListener("resync", () => {
    callbacks.onResync();
  });

  eventSource.addEventListener("update_available", (e: MessageEvent) => {
    const data = JSON.parse(e.data);
    callbacks.onUpdateAvailable(data.version, data.current_version);
  });

  eventSource.onopen = () => {
    callbacks.onConnectionChange(true);
    callbacks.onResync();
  };

  eventSource.onerror = () => {
    callbacks.onConnectionChange(false);
  };
}

export function destroySSE(): void {
  if (eventSource) {
    eventSource.close();
    eventSource = null;
  }
}
