import { writable } from "svelte/store";
import type { StreamEntry, StreamMetrics } from "./api";

export const streamsStore = writable<StreamEntry[]>([]);
export const metricsStore = writable<Record<string, StreamMetrics>>({});

export function addOrUpdateStream(stream: StreamEntry): void {
  streamsStore.update((streams) => {
    const idx = streams.findIndex((s) => s.stream_id === stream.stream_id);
    if (idx >= 0) {
      const updated = [...streams];
      updated[idx] = stream;
      return updated;
    }
    return [...streams, stream];
  });
}

export function patchStream(
  streamId: string,
  fields: Partial<StreamEntry>,
): void {
  streamsStore.update((streams) =>
    streams.map((s) =>
      s.stream_id === streamId ? { ...s, ...fields } : s,
    ),
  );
}

export function setMetrics(streamId: string, metrics: StreamMetrics): void {
  metricsStore.update((m) => ({ ...m, [streamId]: metrics }));
}

export function resetStores(): void {
  streamsStore.set([]);
  metricsStore.set({});
}
