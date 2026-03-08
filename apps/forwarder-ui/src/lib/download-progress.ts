// apps/forwarder-ui/src/lib/download-progress.ts

export type DownloadProgressEvent =
  | {
      state: "downloading";
      progress: number;
      total: number;
      reads_received: number;
    }
  | { state: "complete"; reads_received: number }
  | { state: "error"; message: string }
  | { state: "idle" };

export interface DownloadProgressHandle {
  close(): void;
}

export function subscribeDownloadProgress(
  ip: string,
  onEvent: (event: DownloadProgressEvent) => void,
  onError?: () => void,
): DownloadProgressHandle {
  const es = new EventSource(`/api/v1/readers/${ip}/download-reads/progress`);

  es.onmessage = (msg) => {
    try {
      const data = JSON.parse(msg.data) as DownloadProgressEvent;
      if (!data || typeof data.state !== "string") {
        throw new Error("missing state field");
      }
      onEvent(data);
      if (data.state === "complete" || data.state === "error") {
        es.close();
      }
    } catch (err) {
      console.error("Failed to parse download progress event:", err, msg.data);
      onError?.();
      es.close();
    }
  };

  es.onerror = (evt) => {
    console.error("Download progress SSE error:", evt);
    onError?.();
    es.close();
  };

  return { close: () => es.close() };
}
