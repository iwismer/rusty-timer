import { describe, expect, it } from "vitest";
import {
  deriveStreamDisplayStatus,
  streamDisplayBadge,
  streamDisplayDotClass,
  streamDisplayLabel,
  type StreamDisplayStatus,
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
  const cases: Array<{
    name: string;
    stream: Partial<StreamEntry>;
    recentActivity: boolean;
    optimisticSubscribing: boolean;
    status: StreamDisplayStatus;
    dotClass: string;
    badge: string | null;
    label: string;
  }> = [
    {
      name: "uses a neutral state for streams without subscription intent",
      stream: { subscribed: false },
      recentActivity: false,
      optimisticSubscribing: false,
      status: "not_subscribed",
      dotClass: "bg-text-muted",
      badge: null,
      label: "Not subscribed",
    },
    {
      name: "treats recent read activity with a down reader as receiving but reader down",
      stream: { reader_connected: false },
      recentActivity: true,
      optimisticSubscribing: false,
      status: "receiving_reader_down",
      dotClass: "bg-status-warn",
      badge: "Reader down",
      label: "Receiving — reader down",
    },
    {
      name: "treats recent read activity with a connected reader as receiving",
      stream: { reader_connected: true },
      recentActivity: true,
      optimisticSubscribing: false,
      status: "receiving",
      dotClass: "bg-status-ok",
      badge: null,
      label: "Receiving",
    },
    {
      name: "treats recent read activity as receiving even before reader status is confirmed",
      stream: {},
      recentActivity: true,
      optimisticSubscribing: false,
      status: "receiving_pending",
      dotClass: "bg-status-ok",
      badge: "Reader status pending",
      label: "Receiving — reader status pending",
    },
    {
      name: "shows subscribed streams as subscribing during the optimistic connect grace",
      stream: {},
      recentActivity: false,
      optimisticSubscribing: true,
      status: "subscribing",
      dotClass: "bg-status-warn",
      badge: "Subscribing…",
      label: "Subscribed — connecting",
    },
    {
      name: "shows online streams with a down reader as receiving but reader down",
      stream: { online: true, reader_connected: false },
      recentActivity: false,
      optimisticSubscribing: false,
      status: "receiving_reader_down",
      dotClass: "bg-status-warn",
      badge: "Reader down",
      label: "Receiving — reader down",
    },
    {
      name: "shows online streams with a connected reader as receiving",
      stream: { online: true, reader_connected: true },
      recentActivity: false,
      optimisticSubscribing: false,
      status: "receiving",
      dotClass: "bg-status-ok",
      badge: null,
      label: "Receiving",
    },
    {
      name: "shows online streams with pending reader status as waiting for data",
      stream: { online: true, reader_connected: null },
      recentActivity: false,
      optimisticSubscribing: false,
      status: "waiting_for_data",
      dotClass: "bg-status-warn",
      badge: "Waiting for data",
      label: "Connected — waiting for data",
    },
    {
      name: "shows unknown online state as subscribing",
      stream: { online: null },
      recentActivity: false,
      optimisticSubscribing: false,
      status: "subscribing",
      dotClass: "bg-status-warn",
      badge: "Subscribing…",
      label: "Subscribed — connecting",
    },
    {
      name: "reserves red for subscribed streams with no data and no connect grace",
      stream: {},
      recentActivity: false,
      optimisticSubscribing: false,
      status: "not_receiving",
      dotClass: "bg-status-err",
      badge: "Not receiving",
      label: "Not receiving",
    },
  ];

  it.each(cases)(
    "$name",
    ({
      stream: streamOverrides,
      recentActivity,
      optimisticSubscribing,
      status,
      dotClass,
      badge,
      label,
    }) => {
      const derived = deriveStreamDisplayStatus(stream(streamOverrides), {
        recentActivity,
        optimisticSubscribing,
      });

      expect(derived).toBe(status);
      expect(streamDisplayDotClass(derived)).toBe(dotClass);
      expect(streamDisplayBadge(derived)).toBe(badge);
      expect(streamDisplayLabel(derived)).toBe(label);
    },
  );
});
