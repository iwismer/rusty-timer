import { writable } from "svelte/store";
import { getParticipants } from "./api";
import { buildChipMap, type ChipMap } from "./chipResolver";

export interface RaceData {
  chipMap: ChipMap;
}

/** race_id → RaceData */
export const raceDataStore = writable<Record<string, RaceData>>({});

const loading = new Set<string>();

export async function ensureRaceDataLoaded(raceId: string): Promise<void> {
  if (loading.has(raceId)) return;

  // Check if already in store
  let alreadyLoaded = false;
  raceDataStore.subscribe((data) => {
    alreadyLoaded = raceId in data;
  })();
  if (alreadyLoaded) return;

  loading.add(raceId);
  try {
    const resp = await getParticipants(raceId);
    const chipMap = buildChipMap(resp.participants);
    raceDataStore.update((data) => ({ ...data, [raceId]: { chipMap } }));
  } catch {
    // Failed to load — will retry on next call
  } finally {
    loading.delete(raceId);
  }
}
