export function pushLogEntry(
  entries: string[],
  next: string,
  maxEntries = 500,
): string[] {
  const normalized = next.trim();
  if (!normalized) return entries;
  const prepended = [normalized, ...entries];
  if (prepended.length <= maxEntries) return prepended;
  return prepended.slice(0, maxEntries);
}
