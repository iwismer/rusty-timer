import { describe, expect, it } from "vitest";
import type { StreamEntry } from "./api";
import { groupStreamsByForwarder } from "./groupStreams";

const STREAM_BASE = {
  online: true,
  stream_epoch: 1,
  created_at: "2026-01-01T00:00:00Z",
};

describe("groupStreamsByForwarder", () => {
  it("uses a non-null forwarder_display_name when any stream in the group has one", () => {
    const streams: StreamEntry[] = [
      {
        stream_id: "s1",
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.1",
        display_alias: null,
        forwarder_display_name: null,
        ...STREAM_BASE,
      },
      {
        stream_id: "s2",
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.2",
        display_alias: null,
        forwarder_display_name: "Start Line",
        ...STREAM_BASE,
      },
    ];

    const groups = groupStreamsByForwarder(streams);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.displayName).toBe("Start Line");
  });
});
