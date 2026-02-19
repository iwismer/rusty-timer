import { createSSE, type SseHandle } from "@rusty-timer/shared-ui/lib/sse";
import type { ReaderStatus } from "./api";

export type ForwarderSseCallbacks = {
  onStatusChanged: (data: {
    ready: boolean;
    uplink_connected: boolean;
    restart_needed: boolean;
  }) => void;
  onReaderUpdated: (reader: ReaderStatus) => void;
  onLogEntry: (entry: string) => void;
  onResync: () => void;
  onConnectionChange: (connected: boolean) => void;
  onUpdateAvailable: (version: string, currentVersion: string) => void;
};

let handle: SseHandle | null = null;

export function initSSE(callbacks: ForwarderSseCallbacks): void {
  if (handle) return;

  handle = createSSE(
    "/api/v1/events",
    {
      status_changed: (data: any) => {
        callbacks.onStatusChanged({
          ready: data.ready,
          uplink_connected: data.uplink_connected,
          restart_needed: data.restart_needed,
        });
      },
      reader_updated: (data: any) => {
        callbacks.onReaderUpdated(data as ReaderStatus);
      },
      log_entry: (data: any) => {
        callbacks.onLogEntry(data.entry);
      },
      resync: () => {
        callbacks.onResync();
      },
      update_available: (data: any) => {
        callbacks.onUpdateAvailable(data.version, data.current_version);
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
