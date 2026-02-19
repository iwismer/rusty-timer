import type { StreamEntry } from "./api";

export interface ForwarderGroup {
  forwarderId: string;
  displayName: string;
  streams: StreamEntry[];
}

export function groupStreamsByForwarder(
  streams: StreamEntry[],
): ForwarderGroup[] {
  const groups = new Map<
    string,
    { displayName: string | null; streams: StreamEntry[] }
  >();

  for (const stream of streams) {
    const existing = groups.get(stream.forwarder_id);
    if (!existing) {
      groups.set(stream.forwarder_id, {
        displayName: stream.forwarder_display_name,
        streams: [stream],
      });
      continue;
    }

    if (!existing.displayName && stream.forwarder_display_name) {
      existing.displayName = stream.forwarder_display_name;
    }
    existing.streams.push(stream);
  }

  return [...groups.entries()].map(([forwarderId, group]) => ({
    forwarderId,
    displayName: group.displayName ?? forwarderId,
    streams: group.streams,
  }));
}
