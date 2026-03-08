import { createSSE, type SseHandle } from "@rusty-timer/shared-ui/lib/sse";
import type { ReaderInfo, ReaderStatus, UpdateStatusResponse } from "./api";

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
  onReaderInfoUpdated: (
    data: { ip: string } & import("./api").ReaderInfo,
  ) => void;
  onUpdateStatusChanged: (status: UpdateStatusResponse) => void;
};

let handle: SseHandle | null = null;

export function initSSE(callbacks: ForwarderSseCallbacks): void {
  if (handle) return;

  handle = createSSE(
    "/api/v1/events",
    {
      status_changed: (data: {
        ready: boolean;
        uplink_connected: boolean;
        restart_needed: boolean;
      }) => {
        callbacks.onStatusChanged(data);
      },
      reader_updated: (data: ReaderStatus) => {
        callbacks.onReaderUpdated(data);
      },
      log_entry: (data: { entry: string }) => {
        callbacks.onLogEntry(data.entry);
      },
      resync: () => {
        callbacks.onResync();
      },
      update_status_changed: (data: { status: UpdateStatusResponse }) => {
        callbacks.onUpdateStatusChanged(data.status);
      },
      reader_info_updated: (data: { ip: string } & ReaderInfo) => {
        callbacks.onReaderInfoUpdated(data);
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
