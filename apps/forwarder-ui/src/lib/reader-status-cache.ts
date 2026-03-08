import type { ForwarderStatus, ReaderInfo } from "./api";

export type ReaderStatusCaches = {
  readerInfoMap: Record<string, ReaderInfo>;
  readerInfoReceivedAt: Record<string, number>;
  readerClockBaseTs: Record<string, number>;
  readerClockBaseLocal: Record<string, number>;
  lastSeenBase: Record<string, number | null>;
  lastSeenReceivedAt: Record<string, number>;
};

export type ReaderInfoUpdate = { ip: string } & ReaderInfo;

function parseReaderClock(iso: string): number {
  const normalized = iso.replace(" ", "T");
  const withZ = normalized.endsWith("Z") ? normalized : normalized + "Z";
  return new Date(withZ).getTime();
}

export function rebuildReaderCachesFromStatus(
  status: ForwarderStatus,
  previous: ReaderStatusCaches,
  now: number,
): ReaderStatusCaches {
  const next: ReaderStatusCaches = {
    readerInfoMap: {},
    readerInfoReceivedAt: {},
    readerClockBaseTs: {},
    readerClockBaseLocal: {},
    lastSeenBase: { ...previous.lastSeenBase },
    lastSeenReceivedAt: { ...previous.lastSeenReceivedAt },
  };

  for (const reader of status.readers) {
    next.lastSeenBase[reader.ip] = reader.last_seen_secs;
    next.lastSeenReceivedAt[reader.ip] = now;

    if (!reader.reader_info) {
      continue;
    }

    next.readerInfoMap[reader.ip] = reader.reader_info;
    next.readerInfoReceivedAt[reader.ip] = now;

    const readerClock = reader.reader_info.clock?.reader_clock;
    if (!readerClock) {
      continue;
    }

    const ts = parseReaderClock(readerClock);
    if (Number.isNaN(ts)) {
      continue;
    }

    next.readerClockBaseTs[reader.ip] = ts;
    next.readerClockBaseLocal[reader.ip] = now;
  }

  return next;
}

export function clearReaderInfoForIp(
  previous: ReaderStatusCaches,
  ip: string,
): ReaderStatusCaches {
  const { [ip]: _, ...readerInfoMap } = previous.readerInfoMap;
  const { [ip]: _a, ...readerInfoReceivedAt } = previous.readerInfoReceivedAt;
  const { [ip]: _b, ...readerClockBaseTs } = previous.readerClockBaseTs;
  const { [ip]: _c, ...readerClockBaseLocal } = previous.readerClockBaseLocal;
  return {
    readerInfoMap,
    readerInfoReceivedAt,
    readerClockBaseTs,
    readerClockBaseLocal,
    lastReadBase: previous.lastReadBase,
    lastReadReceivedAt: previous.lastReadReceivedAt,
  };
}

export function applyReaderInfoUpdate(
  status: ForwarderStatus | null,
  previous: ReaderStatusCaches,
  update: ReaderInfoUpdate,
  now: number,
): ReaderStatusCaches {
  const { ip, ...info } = update;
  const reader = status?.readers.find((candidate) => candidate.ip === ip);
  if (reader?.state === "disconnected") {
    return previous;
  }

  const next: ReaderStatusCaches = {
    readerInfoMap: {
      ...previous.readerInfoMap,
      [ip]: {
        ...previous.readerInfoMap[ip],
        ...info,
      },
    },
    readerInfoReceivedAt: {
      ...previous.readerInfoReceivedAt,
      [ip]: now,
    },
    readerClockBaseTs: { ...previous.readerClockBaseTs },
    readerClockBaseLocal: { ...previous.readerClockBaseLocal },
    lastReadBase: previous.lastReadBase,
    lastReadReceivedAt: previous.lastReadReceivedAt,
  };

  const readerClock = info.clock?.reader_clock;
  if (!readerClock) {
    return next;
  }

  const ts = parseReaderClock(readerClock);
  if (Number.isNaN(ts)) {
    return next;
  }

  next.readerClockBaseTs[ip] = ts;
  next.readerClockBaseLocal[ip] = now;
  return next;
}
