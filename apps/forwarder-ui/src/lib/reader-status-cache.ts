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

export function applyReaderInfoUpdate(
  status: ForwarderStatus | null,
  previous: ReaderStatusCaches,
  update: ReaderInfoUpdate,
  now: number,
): ReaderStatusCaches {
  const reader = status?.readers.find(
    (candidate) => candidate.ip === update.ip,
  );
  if (reader?.state === "disconnected") {
    return previous;
  }

  const next: ReaderStatusCaches = {
    readerInfoMap: {
      ...previous.readerInfoMap,
      [update.ip]: {
        ...previous.readerInfoMap[update.ip],
        ...update,
      },
    },
    readerInfoReceivedAt: {
      ...previous.readerInfoReceivedAt,
      [update.ip]: now,
    },
    readerClockBaseTs: { ...previous.readerClockBaseTs },
    readerClockBaseLocal: { ...previous.readerClockBaseLocal },
    lastReadBase: previous.lastReadBase,
    lastReadReceivedAt: previous.lastReadReceivedAt,
  };

  const readerClock = update.clock?.reader_clock;
  if (!readerClock) {
    return next;
  }

  const ts = parseReaderClock(readerClock);
  if (Number.isNaN(ts)) {
    return next;
  }

  next.readerClockBaseTs[update.ip] = ts;
  next.readerClockBaseLocal[update.ip] = now;
  return next;
}
