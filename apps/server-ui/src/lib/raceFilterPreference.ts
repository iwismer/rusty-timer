const RACE_FILTER_KEY = "raceFilter";

export function readRaceFilterPreference(): string | null {
  try {
    if (typeof localStorage === "undefined") return null;
    return localStorage.getItem(RACE_FILTER_KEY);
  } catch {
    return null;
  }
}

export function writeRaceFilterPreference(value: string | null): void {
  try {
    if (typeof localStorage === "undefined") return;
    if (value === null) {
      localStorage.removeItem(RACE_FILTER_KEY);
    } else {
      localStorage.setItem(RACE_FILTER_KEY, value);
    }
  } catch {
    // Ignore storage failures (private mode, blocked storage, quota, etc.).
  }
}
