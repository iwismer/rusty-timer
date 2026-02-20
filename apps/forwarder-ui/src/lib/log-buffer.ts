export function pushLogEntry(
  entries: string[],
  next: string,
  maxEntries = 200,
): string[] {
  const normalized = next.trim();
  if (!normalized) return entries;
  const appended = [...entries, normalized];
  if (appended.length <= maxEntries) return appended;
  return appended.slice(appended.length - maxEntries);
}
