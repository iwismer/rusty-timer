import { describe, it, expect, vi, beforeEach } from "vitest";

// Mock fetch before importing the module under test
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

describe("server_api client", () => {
  // ----- getStreams -----
  it("getStreams calls GET /api/v1/streams and returns stream list", async () => {
    const { getStreams } = await import("./api");
    const payload = {
      streams: [
        {
          stream_id: "abc-123",
          forwarder_id: "fwd-1",
          reader_ip: "192.168.1.100",
          display_alias: "Main reader",
          online: true,
          stream_epoch: 1,
          created_at: "2024-01-01T00:00:00Z",
        },
      ],
    };
    mockFetch.mockResolvedValue(makeResponse(200, payload));
    const result = await getStreams();
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/streams"),
      expect.any(Object),
    );
    expect(result.streams).toHaveLength(1);
    expect(result.streams[0].stream_id).toBe("abc-123");
    expect(result.streams[0].online).toBe(true);
  });

  it("getStreams throws on 500", async () => {
    const { getStreams } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(500, { code: "INTERNAL", message: "oops" }),
    );
    await expect(getStreams()).rejects.toThrow();
  });

  // ----- renameStream -----
  it("renameStream sends PATCH /api/v1/streams/{id} with display_alias body", async () => {
    const { renameStream } = await import("./api");
    const updated = {
      stream_id: "abc-123",
      forwarder_id: "fwd-1",
      reader_ip: "192.168.1.100",
      display_alias: "New Name",
      online: true,
      stream_epoch: 1,
      created_at: "2024-01-01T00:00:00Z",
    };
    mockFetch.mockResolvedValue(makeResponse(200, updated));
    const result = await renameStream("abc-123", "New Name");
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/streams/abc-123"),
      expect.objectContaining({ method: "PATCH" }),
    );
    const callInit = mockFetch.mock.calls[0][1] as RequestInit;
    const body = JSON.parse(callInit.body as string);
    expect(body.display_alias).toBe("New Name");
    expect(result.display_alias).toBe("New Name");
  });

  it("renameStream throws on 404", async () => {
    const { renameStream } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(404, { code: "NOT_FOUND", message: "not found" }),
    );
    await expect(renameStream("bad-id", "X")).rejects.toThrow();
  });

  // ----- getMetrics -----
  it("getMetrics calls GET /api/v1/streams/{id}/metrics and returns metrics", async () => {
    const { getMetrics } = await import("./api");
    const metrics = {
      raw_count: 100,
      dedup_count: 90,
      retransmit_count: 10,
      lag: 1500,
      backlog: 5,
    };
    mockFetch.mockResolvedValue(makeResponse(200, metrics));
    const result = await getMetrics("abc-123");
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/streams/abc-123/metrics"),
      expect.any(Object),
    );
    expect(result.raw_count).toBe(100);
    expect(result.dedup_count).toBe(90);
    expect(result.retransmit_count).toBe(10);
    expect(result.lag).toBe(1500);
    expect(result.backlog).toBe(5);
  });

  it("getMetrics accepts null lag (no events yet)", async () => {
    const { getMetrics } = await import("./api");
    const metrics = {
      raw_count: 0,
      dedup_count: 0,
      retransmit_count: 0,
      lag: null,
      backlog: 0,
    };
    mockFetch.mockResolvedValue(makeResponse(200, metrics));
    const result = await getMetrics("abc-123");
    expect(result.lag).toBeNull();
  });

  it("getMetrics throws on 404", async () => {
    const { getMetrics } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(404, { code: "NOT_FOUND", message: "not found" }),
    );
    await expect(getMetrics("bad-id")).rejects.toThrow();
  });

  // ----- exportRawUrl / exportCsvUrl -----
  it("exportRawUrl returns correct URL for stream", async () => {
    const { exportRawUrl } = await import("./api");
    const url = exportRawUrl("abc-123");
    expect(url).toContain("/api/v1/streams/abc-123/export.txt");
  });

  it("exportCsvUrl returns correct URL for stream", async () => {
    const { exportCsvUrl } = await import("./api");
    const url = exportCsvUrl("abc-123");
    expect(url).toContain("/api/v1/streams/abc-123/export.csv");
  });

  // ----- resetEpoch -----
  it("resetEpoch sends POST /api/v1/streams/{id}/reset-epoch and succeeds on 204", async () => {
    const { resetEpoch } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: true,
      status: 204,
      json: async () => undefined,
      text: async () => "",
    });
    await expect(resetEpoch("abc-123")).resolves.toBeUndefined();
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/streams/abc-123/reset-epoch"),
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("resetEpoch throws on 409 (forwarder not connected)", async () => {
    const { resetEpoch } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(409, {
        code: "FORWARDER_NOT_CONNECTED",
        message: "not connected",
      }),
    );
    await expect(resetEpoch("abc-123")).rejects.toThrow();
  });

  it("resetEpoch throws on 404", async () => {
    const { resetEpoch } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(404, { code: "NOT_FOUND", message: "not found" }),
    );
    await expect(resetEpoch("abc-123")).rejects.toThrow();
  });
});
