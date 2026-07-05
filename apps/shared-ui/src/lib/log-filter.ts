export const LOG_LEVELS = ["trace", "debug", "info", "warn", "error"] as const;
export type LogLevel = (typeof LOG_LEVELS)[number];

export function levelPriority(level: LogLevel): number {
  return LOG_LEVELS.indexOf(level);
}

/** Extract level from "[LEVEL]" tag in entry, default to "info" for untagged entries. */
export function parseLogLevel(entry: string): LogLevel {
  const match = entry.match(/^\d{2}:\d{2}:\d{2} \[(\w+)\]/);
  if (match) {
    const tag = match[1].toLowerCase() as LogLevel;
    if (LOG_LEVELS.includes(tag)) return tag;
  }
  return "info";
}

export function filterEntries(
  entries: string[],
  minLevel: LogLevel,
): string[] {
  const min = levelPriority(minLevel);
  return entries.filter((e) => levelPriority(parseLogLevel(e)) >= min);
}
