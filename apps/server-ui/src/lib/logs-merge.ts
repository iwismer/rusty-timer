export function mergeLogsWithPendingLive(
  snapshot: string[],
  pendingLive: string[],
  maxEntries = 500,
): string[] {
  const newEntries = pendingLive.filter((entry) => !snapshot.includes(entry));
  const merged = [...newEntries.reverse(), ...snapshot];
  return merged.length <= maxEntries ? merged : merged.slice(0, maxEntries);
}
