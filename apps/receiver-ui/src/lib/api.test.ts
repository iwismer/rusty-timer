import { describe, it, expect, vi, beforeEach } from "vitest";

// Mock fetch
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

describe("api client", () => {
  it("getProfile calls correct URL", async () => {
    const { getProfile } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, {
        server_url: "wss://s.com",
        token: "tok",
        log_level: "info",
      }),
    );
    const p = await getProfile();
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/profile",
      expect.any(Object),
    );
    expect(p.server_url).toBe("wss://s.com");
  });

  it("putProfile sends PUT with body", async () => {
    const { putProfile } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(204, null));
    await putProfile({
      server_url: "wss://s.com",
      token: "t",
      log_level: "info",
    });
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/profile",
      expect.objectContaining({ method: "PUT" }),
    );
  });

  it("getStreams returns streams response", async () => {
    const { getStreams } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, { streams: [], degraded: false, upstream_error: null }),
    );
    const r = await getStreams();
    expect(r.degraded).toBe(false);
    expect(r.streams).toEqual([]);
  });

  it("getStatus returns status", async () => {
    const { getStatus } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, {
        connection_state: "disconnected",
        local_ok: true,
        streams_count: 0,
      }),
    );
    const s = await getStatus();
    expect(s.connection_state).toBe("disconnected");
  });

  it("connect accepts 200 or 202", async () => {
    const { connect } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: false,
      status: 202,
      json: async () => ({}),
      text: async () => "",
    });
    await expect(connect()).resolves.toBeUndefined();
  });

  it("disconnect accepts 200 or 202", async () => {
    const { disconnect } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: false,
      status: 200,
      json: async () => ({}),
      text: async () => "",
    });
    await expect(disconnect()).resolves.toBeUndefined();
  });

  it("putSubscriptions sends PUT with subscriptions body", async () => {
    const { putSubscriptions } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(204, null));
    await putSubscriptions([
      {
        forwarder_id: "f",
        reader_ip: "192.168.1.100:10000",
        local_port_override: null,
      },
    ]);
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/subscriptions",
      expect.objectContaining({ method: "PUT" }),
    );
  });

  it("throws on non-ok response", async () => {
    const { getProfile } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(500, "internal error"));
    await expect(getProfile()).rejects.toThrow();
  });
});
