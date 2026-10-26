import type { AnnouncerRow } from "$lib/api";

export const DEFAULT_EXPIRY_MINUTES = 15;
export const MIN_EXPIRY_MINUTES = 1;
export const MAX_EXPIRY_MINUTES = 1440;
export const EXPIRY_STORAGE_KEY = "rusty-timer-announcer-expiry-minutes";

export function clampExpiryMinutes(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_EXPIRY_MINUTES;
  const rounded = Math.round(value);
  if (rounded < MIN_EXPIRY_MINUTES) return MIN_EXPIRY_MINUTES;
  if (rounded > MAX_EXPIRY_MINUTES) return MAX_EXPIRY_MINUTES;
  return rounded;
}

export function loadExpiryMinutes(
  storage?: Pick<Storage, "getItem"> | null,
): number {
  try {
    const target =
      storage !== undefined
        ? storage
        : typeof localStorage !== "undefined"
          ? localStorage
          : null;
    const raw = target?.getItem(EXPIRY_STORAGE_KEY);
    if (!raw) return DEFAULT_EXPIRY_MINUTES;
    const parsed = Number(raw);
    return Number.isFinite(parsed) && parsed >= MIN_EXPIRY_MINUTES
      ? clampExpiryMinutes(parsed)
      : DEFAULT_EXPIRY_MINUTES;
  } catch {
    return DEFAULT_EXPIRY_MINUTES;
  }
}

export function saveExpiryMinutes(
  minutes: number,
  storage?: Pick<Storage, "setItem"> | null,
): number {
  const clamped = clampExpiryMinutes(minutes);
  try {
    const target =
      storage !== undefined
        ? storage
        : typeof localStorage !== "undefined"
          ? localStorage
          : null;
    target?.setItem(EXPIRY_STORAGE_KEY, String(clamped));
  } catch {
    // Ignore storage errors (e.g., private browsing or restricted storage).
  }
  return clamped;
}

export function compareRowsNewestFirst(
  a: AnnouncerRow,
  b: AnnouncerRow,
): number {
  return Date.parse(b.received_at) - Date.parse(a.received_at);
}

export function isRowWithinExpiry(
  row: AnnouncerRow,
  nowMs: number,
  expiryMinutes: number,
): boolean {
  const receivedMs = Date.parse(row.received_at);
  if (Number.isNaN(receivedMs)) return true;
  const maxAgeMs = clampExpiryMinutes(expiryMinutes) * 60_000;
  return nowMs - receivedMs <= maxAgeMs;
}

export function filterVisibleAnnouncerRows(
  rows: AnnouncerRow[],
  nowMs: number,
  expiryMinutes: number,
): AnnouncerRow[] {
  return rows
    .filter((row) => isRowWithinExpiry(row, nowMs, expiryMinutes))
    .sort(compareRowsNewestFirst);
}
