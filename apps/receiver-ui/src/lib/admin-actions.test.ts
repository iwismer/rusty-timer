import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  getStreams: vi.fn().mockResolvedValue({
    streams: [],
    degraded: false,
    upstream_error: null,
  }),
  getSubscriptions: vi.fn().mockResolvedValue({ subscriptions: [] }),
}));

vi.mock("$lib/api", () => apiMocks);

import {
  AdminActions,
  PORT_VALIDATION_MESSAGE,
  portSavedFeedback,
  streamKey,
  streamLabel,
  validatePortInput,
} from "./admin-actions.svelte";

describe("streamKey / streamLabel", () => {
  it("builds a canonical key from endpoint and stream ids", () => {
    expect(streamKey({ forwarder_endpoint_id: "ep-1", stream_id: "s-1" })).toBe(
      "ep-1/s-1",
    );
  });

  it("prefers display_alias, then forwarder/reader, then stream_id", () => {
    const base = {
      forwarder_endpoint_id: "ep-1",
      stream_id: "s-1",
      subscribed: true,
    };
    expect(
      streamLabel({
        ...base,
        forwarder_id: "f1",
        reader_ip: "10.0.0.1:10000",
        display_alias: "Finish",
      } as never),
    ).toBe("Finish");
    expect(
      streamLabel({
        ...base,
        forwarder_id: "f1",
        reader_ip: "10.0.0.1:10000",
      } as never),
    ).toBe("f1 / 10.0.0.1:10000");
    expect(streamLabel({ ...base } as never)).toBe("s-1");
  });
});

describe("validatePortInput", () => {
  it("treats empty or whitespace-only input as clearing the override", () => {
    expect(validatePortInput("")).toEqual({ ok: true, port: null });
    expect(validatePortInput("   ")).toEqual({ ok: true, port: null });
  });

  it("accepts integers in 1-65535 (with surrounding whitespace)", () => {
    expect(validatePortInput("1")).toEqual({ ok: true, port: 1 });
    expect(validatePortInput("8080")).toEqual({ ok: true, port: 8080 });
    expect(validatePortInput("65535")).toEqual({ ok: true, port: 65535 });
    expect(validatePortInput(" 9000 ")).toEqual({ ok: true, port: 9000 });
  });

  it("rejects non-numeric, negative, fractional, and out-of-range values", () => {
    for (const raw of ["9000abc", "-1", "1.5", "0", "65536", "port"]) {
      expect(validatePortInput(raw)).toEqual({
        ok: false,
        message: PORT_VALIDATION_MESSAGE,
      });
    }
  });
});

describe("portSavedFeedback", () => {
  const sub = { forwarder_id: "f1", reader_ip: "10.0.0.1:10000" };

  it("reports 'set' when a port value is present", () => {
    expect(portSavedFeedback(8080, sub)).toBe(
      "Port override set to 8080 for f1 / 10.0.0.1:10000.",
    );
  });

  it("reports 'cleared' only for null (not for falsy values)", () => {
    expect(portSavedFeedback(null, sub)).toBe(
      "Port override cleared for f1 / 10.0.0.1:10000.",
    );
  });
});

describe("AdminActions.bulkAction", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiMocks.getStreams.mockResolvedValue({
      streams: [],
      degraded: false,
      upstream_error: null,
    });
    apiMocks.getSubscriptions.mockResolvedValue({ subscriptions: [] });
  });

  it("invokes afterMutate after a successful bulk action", async () => {
    const afterMutate = vi.fn();
    const actions = new AdminActions({ afterMutate });

    await actions.bulkAction(
      () => Promise.resolve({ deleted: 3 }),
      "Purge subscriptions",
      "purge-subs",
    );

    expect(afterMutate).toHaveBeenCalledTimes(1);
    expect(afterMutate).toHaveBeenCalledWith(undefined);
    expect(actions.feedback).toEqual({
      message: "Purge subscriptions: 3 item(s) removed.",
      ok: true,
    });
    expect(actions.inFlightAction).toBeNull();
  });

  it("passes forceHydrateMode through to afterMutate", async () => {
    const afterMutate = vi.fn();
    const actions = new AdminActions({ afterMutate });

    await actions.bulkAction(
      () => Promise.resolve(),
      "Clear data",
      "clear-data",
      { forceHydrateMode: true },
    );

    expect(afterMutate).toHaveBeenCalledWith({ forceHydrateMode: true });
    expect(actions.feedback).toEqual({
      message: "Clear data: done.",
      ok: true,
    });
  });

  it("does not invoke afterMutate when the action fails", async () => {
    const afterMutate = vi.fn();
    const actions = new AdminActions({ afterMutate });

    await actions.bulkAction(
      () => Promise.reject(new Error("boom")),
      "Factory reset",
      "factory-reset",
    );

    expect(afterMutate).not.toHaveBeenCalled();
    expect(actions.feedback).toEqual({
      message: "Factory reset: failed.",
      ok: false,
    });
    expect(actions.inFlightAction).toBeNull();
  });

  it("works without afterMutate (standalone admin route)", async () => {
    const actions = new AdminActions();

    await actions.bulkAction(
      () => Promise.resolve(),
      "Reset all cursors",
      "reset-all-cursors",
    );

    expect(actions.feedback).toEqual({
      message: "Reset all cursors: done.",
      ok: true,
    });
  });
});
