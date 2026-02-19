import { describe, it, expect, vi, beforeEach } from "vitest";

const mockFetch = vi.fn();
vi.stubGlobal("fetch", mockFetch);

beforeEach(() => {
  mockFetch.mockReset();
});

function makeResponse(status: number, body: unknown) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => JSON.stringify(body),
  };
}

describe("forwarder api client", () => {
  it("getStatus fetches forwarder status", async () => {
    const { getStatus } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, {
        forwarder_id: "fwd-abc",
        version: "0.1.0",
        ready: true,
        ready_reason: null,
        uplink_connected: true,
        restart_needed: false,
        readers: [],
      }),
    );
    const s = await getStatus();
    expect(s.forwarder_id).toBe("fwd-abc");
    expect(s.ready).toBe(true);
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/status",
      expect.any(Object),
    );
  });

  it("getConfig fetches full config JSON", async () => {
    const { getConfig } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, { display_name: "Start Line" }),
    );
    const c = await getConfig();
    expect(c.display_name).toBe("Start Line");
  });

  it("saveConfigSection posts to correct endpoint", async () => {
    const { saveConfigSection } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { ok: true }));
    await saveConfigSection("server", { base_url: "https://s.com" });
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/config/server",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("restart calls restart endpoint", async () => {
    const { restart } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { ok: true }));
    await restart();
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/restart",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("resetEpoch calls correct endpoint", async () => {
    const { resetEpoch } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { new_epoch: 2 }));
    await resetEpoch("192.168.1.10");
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/streams/192.168.1.10/reset-epoch",
      expect.objectContaining({ method: "POST" }),
    );
  });
});
