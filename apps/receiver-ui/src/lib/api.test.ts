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
        update_mode: "check-and-download",
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
      update_mode: "check-and-download",
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

  it("getUpdateStatus calls update status endpoint", async () => {
    const { getUpdateStatus } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, { status: "downloaded", version: "1.2.3" }),
    );
    const status = await getUpdateStatus();
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/update/status",
      expect.any(Object),
    );
    expect(status.status).toBe("downloaded");
    expect(status.version).toBe("1.2.3");
  });

  it("applyUpdate succeeds on 200", async () => {
    const { applyUpdate } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: true,
      status: 200,
      json: async () => ({}),
      text: async () => "",
    });
    await expect(applyUpdate()).resolves.toBeUndefined();
  });

  it("applyUpdate throws on non-200", async () => {
    const { applyUpdate } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: false,
      status: 500,
      json: async () => ({}),
      text: async () => "err",
    });
    await expect(applyUpdate()).rejects.toThrow("apply update -> 500");
  });

  it("checkForUpdate calls POST /api/v1/update/check", async () => {
    const { checkForUpdate } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { status: "up_to_date" }));
    const result = await checkForUpdate();
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/update/check",
      expect.objectContaining({ method: "POST" }),
    );
    expect(result.status).toBe("up_to_date");
  });

  it("downloadUpdate returns failed status payload on 409", async () => {
    const { downloadUpdate } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(409, { status: "failed", error: "no update available" }),
    );
    const result = await downloadUpdate();
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/update/download",
      expect.objectContaining({ method: "POST" }),
    );
    expect(result).toEqual({
      status: "failed",
      error: "no update available",
    });
  });

  it("getSelection calls selection endpoint", async () => {
    const { getSelection } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, {
        selection: { mode: "manual", streams: [] },
        replay_policy: "resume",
      }),
    );
    const result = await getSelection();
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/selection",
      expect.any(Object),
    );
    expect(result.replay_policy).toBe("resume");
  });

  it("putSelection preserves default manual/resume payload shape", async () => {
    const { putSelection } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(204, null));
    const payload: Parameters<typeof putSelection>[0] = {
      selection: {
        mode: "manual",
        streams: [],
      },
      replay_policy: "resume",
    };

    await putSelection(payload);

    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/selection",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify(payload),
      }),
    );

    const [, options] = mockFetch.mock.calls.at(-1)!;
    const body = JSON.parse((options as RequestInit).body as string);
    expect(body).toEqual(payload);
    expect(body.selection).not.toHaveProperty("race_id");
    expect(body.selection).not.toHaveProperty("epoch_scope");
  });

  it("putSelection supports explicit race/current opt-in payload shape", async () => {
    const { putSelection } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(204, null));
    const payload: Parameters<typeof putSelection>[0] = {
      selection: {
        mode: "race",
        race_id: "race-1",
        epoch_scope: "current",
      },
      replay_policy: "live_only",
    };
    await putSelection(payload);
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/selection",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify(payload),
      }),
    );

    const [, options] = mockFetch.mock.calls.at(-1)!;
    const body = JSON.parse((options as RequestInit).body as string);
    expect(body).toEqual(payload);
    expect(body.selection).toMatchObject({
      mode: "race",
      race_id: "race-1",
      epoch_scope: "current",
    });
  });

  it("getRaces calls races endpoint", async () => {
    const { getRaces } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, {
        races: [{ race_id: "r1", name: "Race 1", created_at: "now" }],
      }),
    );
    const result = await getRaces();
    expect(mockFetch).toHaveBeenCalledWith("/api/v1/races", expect.any(Object));
    expect(result.races[0].race_id).toBe("r1");
  });

  it("resetStreamCursor posts admin cursor reset payload", async () => {
    const { resetStreamCursor } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(204, null));

    await resetStreamCursor({
      forwarder_id: "f1",
      reader_ip: "10.0.0.1:10000",
    });

    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/admin/cursors/reset",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({
          forwarder_id: "f1",
          reader_ip: "10.0.0.1:10000",
        }),
      }),
    );

    const [, options] = mockFetch.mock.calls.at(-1)!;
    const headers = new Headers((options as RequestInit).headers);
    expect(headers.get("x-rt-receiver-admin-intent")).toBe(
      "reset-stream-cursor",
    );
  });
});

class MockEventSource {
  static lastInstance: MockEventSource | null = null;

  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  private listeners = new Map<
    string,
    Array<(event: { data: string }) => void>
  >();

  constructor(public readonly url: string) {
    MockEventSource.lastInstance = this;
  }

  addEventListener(type: string, listener: (event: { data: string }) => void) {
    const current = this.listeners.get(type) ?? [];
    current.push(listener);
    this.listeners.set(type, current);
  }

  close() {}

  emit(type: string, payload: unknown) {
    const event = { data: JSON.stringify(payload) };
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event);
    }
  }
}

describe("sse client", () => {
  beforeEach(() => {
    vi.resetModules();
    MockEventSource.lastInstance = null;
    vi.stubGlobal(
      "EventSource",
      MockEventSource as unknown as typeof EventSource,
    );
  });

  it("forwards update_status_changed event payload", async () => {
    const { initSSE, destroySSE } = await import("./sse");
    const callbacks = {
      onStatusChanged: vi.fn(),
      onStreamsSnapshot: vi.fn(),
      onLogEntry: vi.fn(),
      onResync: vi.fn(),
      onConnectionChange: vi.fn(),
      onUpdateStatusChanged: vi.fn(),
      onStreamCountsUpdated: vi.fn(),
    };

    initSSE(callbacks);
    expect(MockEventSource.lastInstance).not.toBeNull();

    MockEventSource.lastInstance!.emit("update_status_changed", {
      status: { status: "available", version: "1.2.3" },
    });

    expect(callbacks.onUpdateStatusChanged).toHaveBeenCalledWith({
      status: "available",
      version: "1.2.3",
    });
    destroySSE();
    vi.unstubAllGlobals();
  });

  it("forwards stream_counts_updated event payload", async () => {
    const { initSSE, destroySSE } = await import("./sse");
    const callbacks = {
      onStatusChanged: vi.fn(),
      onStreamsSnapshot: vi.fn(),
      onLogEntry: vi.fn(),
      onResync: vi.fn(),
      onConnectionChange: vi.fn(),
      onUpdateStatusChanged: vi.fn(),
      onStreamCountsUpdated: vi.fn(),
    };

    initSSE(callbacks);
    expect(MockEventSource.lastInstance).not.toBeNull();

    MockEventSource.lastInstance!.emit("stream_counts_updated", {
      updates: [
        {
          forwarder_id: "f1",
          reader_ip: "10.0.0.1",
          reads_total: 15,
          reads_epoch: 3,
        },
      ],
    });

    expect(callbacks.onStreamCountsUpdated).toHaveBeenCalledWith([
      {
        forwarder_id: "f1",
        reader_ip: "10.0.0.1",
        reads_total: 15,
        reads_epoch: 3,
      },
    ]);

    destroySSE();
    vi.unstubAllGlobals();
  });
});
