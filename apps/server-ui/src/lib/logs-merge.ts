export function mergeLogsWithPendingLive(
  snapshot: string[],
  pendingLive: string[],
  maxEntries = 500,
): string[] {
  const merged = [...snapshot];
  for (const entry of pendingLive) {
    if (!merged.includes(entry)) {
      merged.push(entry);
    }
  }
  return merged.length <= maxEntries
    ? merged
    : merged.slice(merged.length - maxEntries);
}
