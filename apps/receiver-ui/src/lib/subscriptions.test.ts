import { describe, expect, it } from "vitest";

import {
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

describe("buildUpdatedSubscriptions", () => {
  it("unsubscribes by removing the target and keeping existing subscribed streams", () => {
    const result = buildUpdatedSubscriptions({
      allStreams: [
        {
          forwarder_id: "f1",
          reader_ip: "10.0.0.1",
          subscribed: true,
          local_port: 10001,
        },
        {
          forwarder_id: "f2",
          reader_ip: "10.0.0.2",
          subscribed: true,
          local_port: null,
        },
        {
          forwarder_id: "f3",
          reader_ip: "10.0.0.3",
          subscribed: false,
          local_port: null,
        },
      ],
      target: {
        forwarder_id: "f1",
        reader_ip: "10.0.0.1",
        currentlySubscribed: true,
      },
    });

    expect(result.error).toBeNull();
    expect(result.subscriptions).toEqual([
      {
        forwarder_id: "f2",
        reader_ip: "10.0.0.2",
        local_port_override: null,
      },
    ]);
  });

  it("subscribes by adding target stream and preserving existing subscribed streams", () => {
    const result = buildUpdatedSubscriptions({
      allStreams: [
        {
          forwarder_id: "f1",
          reader_ip: "10.0.0.1",
          subscribed: true,
          local_port: 10001,
        },
        {
          forwarder_id: "f2",
          reader_ip: "10.0.0.2",
          subscribed: false,
          local_port: null,
        },
      ],
      target: {
        forwarder_id: "f2",
        reader_ip: "10.0.0.2",
        currentlySubscribed: false,
      },
      rawPortOverride: "9002",
    });

    expect(result.error).toBeNull();
    expect(result.subscriptions).toEqual([
      {
        forwarder_id: "f1",
        reader_ip: "10.0.0.1",
        local_port_override: 10001,
      },
      {
        forwarder_id: "f2",
        reader_ip: "10.0.0.2",
        local_port_override: 9002,
      },
    ]);
  });

  it("returns validation error for invalid subscribe port override", () => {
    const result = buildUpdatedSubscriptions({
      allStreams: [
        {
          forwarder_id: "f1",
          reader_ip: "10.0.0.1",
          subscribed: false,
          local_port: null,
        },
      ],
      target: {
        forwarder_id: "f1",
        reader_ip: "10.0.0.1",
        currentlySubscribed: false,
      },
      rawPortOverride: "70000",
    });

    expect(result.subscriptions).toBeNull();
    expect(result.error).toBe("Port override must be in range 1-65535.");
  });
});
