const HIDE_OFFLINE_KEY = "hideOffline";

export function readHideOfflinePreference(): boolean {
  try {
    if (typeof localStorage === "undefined") return false;
    return localStorage.getItem(HIDE_OFFLINE_KEY) === "true";
  } catch {
    return false;
  }
}

export function writeHideOfflinePreference(value: boolean): void {
  try {
    if (typeof localStorage === "undefined") return;
    localStorage.setItem(HIDE_OFFLINE_KEY, String(value));
  } catch {
    // Ignore storage failures (private mode, blocked storage, quota, etc.).
  }
}
