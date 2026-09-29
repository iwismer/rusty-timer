export const LOG_LEVELS = ["trace", "debug", "info", "warn", "error"] as const;
export type LogLevel = (typeof LOG_LEVELS)[number];

export interface ParsedLogEntry {
  raw: string;
  timestamp: string;
  level: LogLevel;
  sourceTag?: string;
  message: string;
}

export function levelPriority(level: LogLevel): number {
  return LOG_LEVELS.indexOf(level);
}

/** Extract level from "[LEVEL]" tag in entry, default to "info" for untagged entries. Supports millisecond timestamps. */
export function parseLogLevel(entry: string): LogLevel {
  const match = entry.match(/^\d{2}:\d{2}:\d{2}(?:\.\d+)? \[(\w+)\]/);
  if (match) {
    const tag = match[1].toLowerCase() as LogLevel;
    if (LOG_LEVELS.includes(tag)) return tag;
  }
  return "info";
}

/** Parse a structured log entry into its components: timestamp, level, sourceTag, and message body. */
export function parseLogEntry(entry: string): ParsedLogEntry {
  const match = entry.match(/^(\d{2}:\d{2}:\d{2}(?:\.\d+)?)\s+\[(\w+)\](?:\s+\[([A-Za-z0-9_:-]+)\])?\s*(.*)$/);
  if (match) {
    const rawTag = match[2].toLowerCase() as LogLevel;
    const level = LOG_LEVELS.includes(rawTag) ? rawTag : "info";
    return {
      raw: entry,
      timestamp: match[1],
      level,
      sourceTag: match[3],
      message: match[4] ?? "",
    };
  }
  // Fallback for untagged or non-standard entries
  const timeMatch = entry.match(/^(\d{2}:\d{2}:\d{2}(?:\.\d+)?)\s*(.*)$/);
  if (timeMatch) {
    return {
      raw: entry,
      timestamp: timeMatch[1],
      level: "info",
      message: timeMatch[2] ?? "",
    };
  }
  return {
    raw: entry,
    timestamp: "",
    level: "info",
    message: entry,
  };
}

/** Count entries for each log level in a collection. */
export function countByLevel(entries: string[]): Record<LogLevel, number> {
  const counts: Record<LogLevel, number> = {
    trace: 0,
    debug: 0,
    info: 0,
    warn: 0,
    error: 0,
  };
  for (const entry of entries) {
    const lvl = parseLogLevel(entry);
    counts[lvl] = (counts[lvl] || 0) + 1;
  }
  return counts;
}

/** Filter entries by minimum level priority (backwards-compatible). */
export function filterEntries(
  entries: string[],
  minLevel: LogLevel,
): string[] {
  const min = levelPriority(minLevel);
  return entries.filter((e) => levelPriority(parseLogLevel(e)) >= min);
}

/** Filter entries by active log levels set and optional text search query. */
export function filterEntriesAdvanced(
  entries: string[],
  activeLevels: Set<LogLevel> | readonly LogLevel[],
  searchQuery?: string,
): string[] {
  const levelSet = activeLevels instanceof Set ? activeLevels : new Set(activeLevels);
  const query = searchQuery ? searchQuery.trim().toLowerCase() : "";

  return entries.filter((entry) => {
    const level = parseLogLevel(entry);
    if (!levelSet.has(level)) {
      return false;
    }
    if (query && !entry.toLowerCase().includes(query)) {
      return false;
    }
    return true;
  });
}
