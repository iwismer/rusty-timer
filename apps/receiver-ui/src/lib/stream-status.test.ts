import { describe, expect, it } from "vitest";
import {
  deriveStreamDisplayStatus,
  streamDisplayBadge,
  streamDisplayDotClass,
} from "./stream-status";
import type { StreamEntry } from "./api";

function stream(overrides: Partial<StreamEntry> = {}): StreamEntry {
  return {
    forwarder_endpoint_id: "endpoint-1",
    stream_id: "stream-1",
    subscribed: true,
    local_port: 10100,
    online: false,
    reader_connected: null,
    reads_total: null,
    ...overrides,
  };
}

describe("stream display status", () => {
  it("treats recent read activity as receiving even before reader status is confirmed", () => {
    const status = deriveStreamDisplayStatus(stream(), {
      recentActivity: true,
      optimisticSubscribing: false,
    });

    expect(status).toBe("receiving_pending");
    expect(streamDisplayDotClass(status)).toBe("bg-status-ok");
    expect(streamDisplayBadge(status)).toBe("Reader status pending");
  });

  it("shows subscribed streams as subscribing during the optimistic connect grace", () => {
    const status = deriveStreamDisplayStatus(stream(), {
      recentActivity: false,
      optimisticSubscribing: true,
    });

    expect(status).toBe("subscribing");
    expect(streamDisplayDotClass(status)).toBe("bg-status-warn");
    expect(streamDisplayBadge(status)).toBe("Subscribing…");
  });

  it("reserves red for subscribed streams with no data and no connect grace", () => {
    const status = deriveStreamDisplayStatus(stream(), {
      recentActivity: false,
      optimisticSubscribing: false,
    });

    expect(status).toBe("not_receiving");
    expect(streamDisplayDotClass(status)).toBe("bg-status-err");
    expect(streamDisplayBadge(status)).toBe("Not receiving");
  });

  it("uses a neutral state for streams without subscription intent", () => {
    const status = deriveStreamDisplayStatus(stream({ subscribed: false }), {
      recentActivity: false,
      optimisticSubscribing: false,
    });

    expect(status).toBe("not_subscribed");
    expect(streamDisplayDotClass(status)).toBe("bg-text-muted");
    expect(streamDisplayBadge(status)).toBeNull();
  });
});
