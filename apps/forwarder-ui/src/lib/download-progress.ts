// apps/forwarder-ui/src/lib/download-progress.ts

export interface DownloadProgressEvent {
  state: "downloading" | "complete" | "error" | "idle";
  progress?: number;
  total?: number;
  reads_received?: number;
  message?: string;
}

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
      const data: DownloadProgressEvent = JSON.parse(msg.data);
      onEvent(data);
      if (data.state === "complete" || data.state === "error") {
        es.close();
      }
    } catch (err) {
      console.error("Failed to parse download progress event:", err, msg.data);
    }
  };

  es.onerror = () => {
    onError?.();
    es.close();
  };

  return { close: () => es.close() };
}
