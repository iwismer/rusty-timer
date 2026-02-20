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
          reader_ip: "192.168.1.100:10000",
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
      reader_ip: "192.168.1.100:10000",
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
      lag_ms: 1500,
      epoch_raw_count: 50,
      epoch_dedup_count: 45,
      epoch_retransmit_count: 5,
      epoch_lag_ms: 800,
      epoch_last_received_at: "2026-02-18T12:00:00Z",
      unique_chips: 10,
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
    expect(result.backlog).toBe(0);
    expect(result.epoch_raw_count).toBe(50);
    expect(result.epoch_dedup_count).toBe(45);
    expect(result.epoch_retransmit_count).toBe(5);
    expect(result.epoch_lag).toBe(800);
    expect(result.epoch_last_received_at).toBe("2026-02-18T12:00:00Z");
    expect(result.unique_chips).toBe(10);
  });

  it("getMetrics accepts null lag (no events yet)", async () => {
    const { getMetrics } = await import("./api");
    const metrics = {
      raw_count: 0,
      dedup_count: 0,
      retransmit_count: 0,
      lag_ms: null,
      epoch_raw_count: 0,
      epoch_dedup_count: 0,
      epoch_retransmit_count: 0,
      epoch_lag_ms: null,
      epoch_last_received_at: null,
      unique_chips: 0,
    };
    mockFetch.mockResolvedValue(makeResponse(200, metrics));
    const result = await getMetrics("abc-123");
    expect(result.lag).toBeNull();
    expect(result.epoch_lag).toBeNull();
    expect(result.epoch_last_received_at).toBeNull();
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

  // ----- Admin: getTokens -----
  it("getTokens calls GET /api/v1/admin/tokens", async () => {
    const { getTokens } = await import("./api");
    const payload = {
      tokens: [
        {
          token_id: "tok-1",
          device_type: "forwarder",
          device_id: "fwd-1",
          created_at: "2026-01-01T00:00:00Z",
          revoked: false,
        },
      ],
    };
    mockFetch.mockResolvedValue(makeResponse(200, payload));
    const result = await getTokens();
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/admin/tokens"),
      expect.any(Object),
    );
    expect(result.tokens).toHaveLength(1);
    expect(result.tokens[0].device_id).toBe("fwd-1");
  });

  // ----- Admin: revokeToken -----
  it("revokeToken sends POST /api/v1/admin/tokens/{id}/revoke", async () => {
    const { revokeToken } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: true,
      status: 204,
      json: async () => undefined,
      text: async () => "",
    });
    await expect(revokeToken("tok-1")).resolves.toBeUndefined();
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/admin/tokens/tok-1/revoke"),
      expect.objectContaining({ method: "POST" }),
    );
  });

  // ----- Admin: createToken -----
  it("createToken sends POST /api/v1/admin/tokens with device info", async () => {
    const { createToken } = await import("./api");
    const created = {
      token_id: "new-tok-1",
      device_id: "my-fwd",
      device_type: "forwarder",
      token: "xK9mP2vQ7nR4sT5uW8yA1bC3dE6fG9hJ2kL4mN7pQ0r",
    };
    mockFetch.mockResolvedValue(makeResponse(201, created));
    const result = await createToken({
      device_id: "my-fwd",
      device_type: "forwarder",
    });
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/admin/tokens"),
      expect.objectContaining({ method: "POST" }),
    );
    const callInit = mockFetch.mock.calls[0][1] as RequestInit;
    const body = JSON.parse(callInit.body as string);
    expect(body.device_id).toBe("my-fwd");
    expect(body.device_type).toBe("forwarder");
    expect(result.token).toBe("xK9mP2vQ7nR4sT5uW8yA1bC3dE6fG9hJ2kL4mN7pQ0r");
  });

  it("createToken with custom token string", async () => {
    const { createToken } = await import("./api");
    const created = {
      token_id: "new-tok-2",
      device_id: "my-rcv",
      device_type: "receiver",
      token: "my-custom-token",
    };
    mockFetch.mockResolvedValue(makeResponse(201, created));
    const result = await createToken({
      device_id: "my-rcv",
      device_type: "receiver",
      token: "my-custom-token",
    });
    const callInit = mockFetch.mock.calls[0][1] as RequestInit;
    const body = JSON.parse(callInit.body as string);
    expect(body.token).toBe("my-custom-token");
    expect(result.token).toBe("my-custom-token");
  });

  // ----- Admin: deleteStream -----
  it("deleteStream sends DELETE /api/v1/admin/streams/{id}", async () => {
    const { deleteStream } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: true,
      status: 204,
      json: async () => undefined,
      text: async () => "",
    });
    await expect(deleteStream("abc-123")).resolves.toBeUndefined();
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/admin/streams/abc-123"),
      expect.objectContaining({ method: "DELETE" }),
    );
  });

  // ----- Admin: deleteAllStreams -----
  it("deleteAllStreams sends DELETE /api/v1/admin/streams", async () => {
    const { deleteAllStreams } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: true,
      status: 204,
      json: async () => undefined,
      text: async () => "",
    });
    await expect(deleteAllStreams()).resolves.toBeUndefined();
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/admin/streams"),
      expect.objectContaining({ method: "DELETE" }),
    );
  });

  // ----- Admin: deleteAllEvents -----
  it("deleteAllEvents sends DELETE /api/v1/admin/events", async () => {
    const { deleteAllEvents } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: true,
      status: 204,
      json: async () => undefined,
      text: async () => "",
    });
    await expect(deleteAllEvents()).resolves.toBeUndefined();
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/admin/events"),
      expect.objectContaining({ method: "DELETE" }),
    );
  });

  // ----- Admin: deleteStreamEvents -----
  it("deleteStreamEvents sends DELETE /api/v1/admin/streams/{id}/events", async () => {
    const { deleteStreamEvents } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: true,
      status: 204,
      json: async () => undefined,
      text: async () => "",
    });
    await expect(deleteStreamEvents("abc-123")).resolves.toBeUndefined();
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/admin/streams/abc-123/events"),
      expect.objectContaining({ method: "DELETE" }),
    );
  });

  // ----- Admin: deleteEpochEvents -----
  it("deleteEpochEvents sends DELETE /api/v1/admin/streams/{id}/epochs/{epoch}/events", async () => {
    const { deleteEpochEvents } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: true,
      status: 204,
      json: async () => undefined,
      text: async () => "",
    });
    await expect(deleteEpochEvents("abc-123", 2)).resolves.toBeUndefined();
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/admin/streams/abc-123/epochs/2/events"),
      expect.objectContaining({ method: "DELETE" }),
    );
  });

  // ----- Admin: deleteAllCursors -----
  it("deleteAllCursors sends DELETE /api/v1/admin/receiver-cursors", async () => {
    const { deleteAllCursors } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: true,
      status: 204,
      json: async () => undefined,
      text: async () => "",
    });
    await expect(deleteAllCursors()).resolves.toBeUndefined();
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/admin/receiver-cursors"),
      expect.objectContaining({ method: "DELETE" }),
    );
  });

  // ----- Admin: getCursors -----
  it("getCursors calls GET /api/v1/admin/receiver-cursors", async () => {
    const { getCursors } = await import("./api");
    const payload = {
      cursors: [
        {
          receiver_id: "rcv-1",
          stream_id: "abc-123",
          stream_epoch: 2,
          last_seq: 10,
          updated_at: "2026-02-20T12:00:00Z",
        },
      ],
    };
    mockFetch.mockResolvedValue(makeResponse(200, payload));
    const result = await getCursors();
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/admin/receiver-cursors"),
      expect.any(Object),
    );
    expect(result.cursors).toHaveLength(1);
    expect(result.cursors[0].receiver_id).toBe("rcv-1");
  });

  // ----- Admin: deleteReceiverCursors -----
  it("deleteReceiverCursors sends DELETE /api/v1/admin/receiver-cursors/{id}", async () => {
    const { deleteReceiverCursors } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: true,
      status: 204,
      json: async () => undefined,
      text: async () => "",
    });
    await expect(deleteReceiverCursors("rcv-1")).resolves.toBeUndefined();
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/admin/receiver-cursors/rcv-1"),
      expect.objectContaining({ method: "DELETE" }),
    );
  });

  // ----- Admin: deleteReceiverStreamCursor -----
  it("deleteReceiverStreamCursor sends DELETE /api/v1/admin/receiver-cursors/{receiverId}/{streamId}", async () => {
    const { deleteReceiverStreamCursor } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: true,
      status: 204,
      json: async () => undefined,
      text: async () => "",
    });
    await expect(
      deleteReceiverStreamCursor("rcv-1", "abc-123"),
    ).resolves.toBeUndefined();
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/admin/receiver-cursors/rcv-1/abc-123"),
      expect.objectContaining({ method: "DELETE" }),
    );
  });

  it("deleteReceiverCursors URL-encodes receiver ID", async () => {
    const { deleteReceiverCursors } = await import("./api");
    mockFetch.mockResolvedValue({
      ok: true,
      status: 204,
      json: async () => undefined,
      text: async () => "",
    });
    await deleteReceiverCursors("rcv/special");
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining("/api/v1/admin/receiver-cursors/rcv%2Fspecial"),
      expect.objectContaining({ method: "DELETE" }),
    );
  });
});
