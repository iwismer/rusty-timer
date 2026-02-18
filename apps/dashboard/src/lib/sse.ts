import {
  addOrUpdateStream,
  patchStream,
  replaceStreams,
  setMetrics,
} from "./stores";
import { getStreams } from "./api";
import type { StreamEntry, StreamMetrics } from "./api";

let eventSource: EventSource | null = null;

export function initSSE(): void {
  if (eventSource) return;

  eventSource = new EventSource("/api/v1/events");

  eventSource.addEventListener("stream_created", (e: MessageEvent) => {
    const stream: StreamEntry = JSON.parse(e.data);
    addOrUpdateStream(stream);
  });

  eventSource.addEventListener("stream_updated", (e: MessageEvent) => {
    const update = JSON.parse(e.data);
    const { stream_id, ...fields } = update;
    patchStream(stream_id, fields);
  });

  eventSource.addEventListener("metrics_updated", (e: MessageEvent) => {
    const data = JSON.parse(e.data);
    const metrics: StreamMetrics = {
      raw_count: data.raw_count,
      dedup_count: data.dedup_count,
      retransmit_count: data.retransmit_count,
      lag: data.lag_ms ?? null,
      backlog: 0,
      epoch_raw_count: data.epoch_raw_count,
      epoch_dedup_count: data.epoch_dedup_count,
      epoch_retransmit_count: data.epoch_retransmit_count,
      epoch_lag: data.epoch_lag_ms ?? null,
      epoch_last_received_at: data.epoch_last_received_at ?? null,
      unique_chips: data.unique_chips,
    };
    setMetrics(data.stream_id, metrics);
  });

  eventSource.addEventListener("resync", async () => {
    await resync();
  });

  eventSource.onopen = async () => {
    await resync();
  };
}

async function resync(): Promise<void> {
  try {
    const resp = await getStreams();
    replaceStreams(resp.streams);
  } catch {
    // Resync failed — SSE will keep trying via auto-reconnect
  }
}

export function destroySSE(): void {
  if (eventSource) {
    eventSource.close();
    eventSource = null;
  }
}
