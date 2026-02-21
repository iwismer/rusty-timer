import { writable } from "svelte/store";
import type { StreamEntry, StreamMetrics, RaceEntry } from "./api";

export const streamsStore = writable<StreamEntry[]>([]);
export const metricsStore = writable<Record<string, StreamMetrics>>({});

/** forwarder_id → race_id (null means unassigned) */
export const forwarderRacesStore = writable<Record<string, string | null>>({});

/** All races for dropdown selection */
export const racesStore = writable<RaceEntry[]>([]);
export const racesLoadedStore = writable(false);

export const logsStore = writable<string[]>([]);

export function pushLog(entry: string): void {
  logsStore.update((entries) => {
    const next = [...entries, entry.trim()];
    return next.length <= 500 ? next : next.slice(next.length - 500);
  });
}

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
    streams.map((s) => (s.stream_id === streamId ? { ...s, ...fields } : s)),
  );
}

export function setMetrics(streamId: string, metrics: StreamMetrics): void {
  metricsStore.update((m) => ({ ...m, [streamId]: metrics }));
}

export function replaceStreams(streams: StreamEntry[]): void {
  streamsStore.set(streams);
}

export function setForwarderRace(
  forwarderId: string,
  raceId: string | null,
): void {
  forwarderRacesStore.update((m) => ({ ...m, [forwarderId]: raceId }));
}

export function setRaces(races: RaceEntry[]): void {
  racesStore.set(races);
  racesLoadedStore.set(true);
}

export function resetStores(): void {
  streamsStore.set([]);
  metricsStore.set({});
  forwarderRacesStore.set({});
  racesStore.set([]);
  racesLoadedStore.set(false);
  logsStore.set([]);
}
