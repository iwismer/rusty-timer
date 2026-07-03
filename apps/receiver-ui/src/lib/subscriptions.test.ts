import { describe, expect, it } from "vitest";

import {
  buildAllSubscriptions,
  buildUpdatedSubscriptions,
  parsePortOverrideInput,
} from "./subscriptions";

describe("parsePortOverrideInput", () => {
  it("returns null override for blank input", () => {
    expect(parsePortOverrideInput("")).toEqual({ value: null, error: null });
    expect(parsePortOverrideInput("   ")).toEqual({ value: null, error: null });
    expect(parsePortOverrideInput(undefined)).toEqual({
      value: null,
      error: null,
    });
  });

  it("accepts integer ports in range 1..65535", () => {
    expect(parsePortOverrideInput("9900")).toEqual({
      value: 9900,
      error: null,
    });
    expect(parsePortOverrideInput("00042")).toEqual({ value: 42, error: null });
    expect(parsePortOverrideInput("65535")).toEqual({
      value: 65535,
      error: null,
    });
  });

  it("accepts numeric runtime values from number inputs", () => {
    expect(parsePortOverrideInput(9002)).toEqual({
      value: 9002,
      error: null,
    });
  });

  it("rejects non-integer input", () => {
    expect(parsePortOverrideInput("10.5")).toEqual({
      value: null,
      error: "Port override must be an integer (1-65535).",
    });
    expect(parsePortOverrideInput("abc")).toEqual({
      value: null,
      error: "Port override must be an integer (1-65535).",
    });
  });

  it("rejects out-of-range values", () => {
    expect(parsePortOverrideInput("0")).toEqual({
      value: null,
      error: "Port override must be in range 1-65535.",
    });
    expect(parsePortOverrideInput("70000")).toEqual({
      value: null,
      error: "Port override must be in range 1-65535.",
    });
  });
});

describe("buildAllSubscriptions", () => {
  it("resubmits the stored override separately from the resolved local port", () => {
    const result = buildAllSubscriptions([
      {
        forwarder_endpoint_id: "endpoint-default",
        stream_id: "10.0.0.1:10000",
        subscribed: true,
        local_port: 10001,
        local_port_override: null,
      },
      {
        forwarder_endpoint_id: "endpoint-explicit",
        stream_id: "10.0.0.2:10000",
        subscribed: true,
        local_port: 9900,
        local_port_override: 9900,
      },
    ]);

    expect(result).toEqual([
      {
        forwarder_endpoint_id: "endpoint-default",
        stream_id: "10.0.0.1:10000",
        local_port_override: null,
        event_type: "finish",
      },
      {
        forwarder_endpoint_id: "endpoint-explicit",
        stream_id: "10.0.0.2:10000",
        local_port_override: 9900,
        event_type: "finish",
      },
    ]);
  });
});

describe("buildUpdatedSubscriptions", () => {
  it("preserves existing stored overrides when subscribing another stream", () => {
    const result = buildUpdatedSubscriptions({
      allStreams: [
        {
          forwarder_endpoint_id: "endpoint-default",
          stream_id: "10.0.0.1:10000",
          subscribed: true,
          local_port: 10001,
          local_port_override: null,
        },
        {
          forwarder_endpoint_id: "endpoint-explicit",
          stream_id: "10.0.0.2:10000",
          subscribed: true,
          local_port: 9900,
          local_port_override: 9900,
        },
        {
          forwarder_endpoint_id: "endpoint-new",
          stream_id: "10.0.0.3:10000",
          subscribed: false,
          local_port: null,
        },
      ],
      target: {
        forwarder_endpoint_id: "endpoint-new",
        stream_id: "10.0.0.3:10000",
        currentlySubscribed: false,
      },
    });

    expect(result.error).toBeNull();
    expect(result.subscriptions).toEqual([
      {
        forwarder_endpoint_id: "endpoint-default",
        stream_id: "10.0.0.1:10000",
        local_port_override: null,
        event_type: "finish",
      },
      {
        forwarder_endpoint_id: "endpoint-explicit",
        stream_id: "10.0.0.2:10000",
        local_port_override: 9900,
        event_type: "finish",
      },
      {
        forwarder_endpoint_id: "endpoint-new",
        stream_id: "10.0.0.3:10000",
        local_port_override: null,
        event_type: "finish",
      },
    ]);
  });

  it("unsubscribes by removing the target and keeping existing subscribed streams", () => {
    const result = buildUpdatedSubscriptions({
      allStreams: [
        {
          forwarder_endpoint_id: "endpoint-1",
          stream_id: "stream-1",
          subscribed: true,
          local_port: 10001,
        },
        {
          forwarder_endpoint_id: "endpoint-2",
          stream_id: "stream-2",
          subscribed: true,
          local_port: null,
        },
        {
          forwarder_endpoint_id: "endpoint-3",
          stream_id: "stream-3",
          subscribed: false,
          local_port: null,
        },
      ],
      target: {
        forwarder_endpoint_id: "endpoint-1",
        stream_id: "stream-1",
        currentlySubscribed: true,
      },
    });

    expect(result.error).toBeNull();
    expect(result.subscriptions).toEqual([
      {
        forwarder_endpoint_id: "endpoint-2",
        stream_id: "stream-2",
        local_port_override: null,
        event_type: "finish",
      },
    ]);
  });

  it("subscribes by adding target stream and preserving existing subscribed streams", () => {
    const result = buildUpdatedSubscriptions({
      allStreams: [
        {
          forwarder_endpoint_id: "endpoint-1",
          stream_id: "stream-1",
          forwarder_id: "legacy-fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 10001,
          local_port_override: 10001,
          event_type: "start",
        },
        {
          forwarder_endpoint_id: "endpoint-2",
          stream_id: "stream-2",
          forwarder_id: "legacy-fwd-2",
          reader_ip: "10.0.0.2:10000",
          subscribed: false,
          local_port: null,
        },
        {
          forwarder_endpoint_id: "endpoint-3",
          stream_id: "stream-3",
          subscribed: false,
          local_port: null,
        },
      ],
      target: {
        forwarder_endpoint_id: "endpoint-2",
        stream_id: "stream-2",
        currentlySubscribed: false,
      },
      rawPortOverride: "9002",
    });

    expect(result.error).toBeNull();
    expect(result.subscriptions).toEqual([
      {
        forwarder_endpoint_id: "endpoint-1",
        stream_id: "stream-1",
        forwarder_id: "legacy-fwd-1",
        reader_ip: "10.0.0.1:10000",
        local_port_override: 10001,
        event_type: "start",
      },
      {
        forwarder_endpoint_id: "endpoint-2",
        stream_id: "stream-2",
        forwarder_id: "legacy-fwd-2",
        reader_ip: "10.0.0.2:10000",
        local_port_override: 9002,
        event_type: "finish",
      },
    ]);
  });

  it("does not fabricate legacy metadata for canonical-only subscriptions", () => {
    const result = buildUpdatedSubscriptions({
      allStreams: [
        {
          forwarder_endpoint_id: "endpoint-1",
          stream_id: "stream-1",
          subscribed: true,
          local_port: null,
        },
        {
          forwarder_endpoint_id: "endpoint-2",
          stream_id: "stream-2",
          subscribed: false,
          local_port: null,
        },
      ],
      target: {
        forwarder_endpoint_id: "endpoint-2",
        stream_id: "stream-2",
        currentlySubscribed: false,
      },
    });

    expect(result.error).toBeNull();
    expect(result.subscriptions).toEqual([
      {
        forwarder_endpoint_id: "endpoint-1",
        stream_id: "stream-1",
        local_port_override: null,
        event_type: "finish",
      },
      {
        forwarder_endpoint_id: "endpoint-2",
        stream_id: "stream-2",
        local_port_override: null,
        event_type: "finish",
      },
    ]);
  });

  it("preserves existing event types when unsubscribing another stream", () => {
    const result = buildUpdatedSubscriptions({
      allStreams: [
        {
          forwarder_endpoint_id: "endpoint-1",
          stream_id: "stream-1",
          subscribed: true,
          local_port: 10001,
          local_port_override: 10001,
          event_type: "start",
        },
        {
          forwarder_endpoint_id: "endpoint-2",
          stream_id: "stream-2",
          subscribed: true,
          local_port: null,
          event_type: "finish",
        },
      ],
      target: {
        forwarder_endpoint_id: "endpoint-2",
        stream_id: "stream-2",
        currentlySubscribed: true,
      },
    });

    expect(result.error).toBeNull();
    expect(result.subscriptions).toEqual([
      {
        forwarder_endpoint_id: "endpoint-1",
        stream_id: "stream-1",
        local_port_override: 10001,
        event_type: "start",
      },
    ]);
  });

  it("returns validation error for invalid subscribe port override", () => {
    const result = buildUpdatedSubscriptions({
      allStreams: [
        {
          forwarder_endpoint_id: "endpoint-1",
          stream_id: "stream-1",
          subscribed: false,
          local_port: null,
        },
      ],
      target: {
        forwarder_endpoint_id: "endpoint-1",
        stream_id: "stream-1",
        currentlySubscribed: false,
      },
      rawPortOverride: "70000",
    });

    expect(result.subscriptions).toBeNull();
    expect(result.error).toBe("Port override must be in range 1-65535.");
  });
});
