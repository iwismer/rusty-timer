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

  it("restartService calls control restart-service endpoint", async () => {
    const { restartService } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { ok: true }));
    await restartService();
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/control/restart-service",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("restartDevice calls control restart-device endpoint", async () => {
    const { restartDevice } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { ok: true }));
    await restartDevice();
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/control/restart-device",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("shutdownDevice calls control shutdown-device endpoint", async () => {
    const { shutdownDevice } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { ok: true }));
    await shutdownDevice();
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/control/shutdown-device",
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

  it("setCurrentEpochName calls correct endpoint with name", async () => {
    const { setCurrentEpochName } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { ok: true }));
    await setCurrentEpochName("192.168.1.10", "Lap 2");
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/streams/192.168.1.10/current-epoch/name",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ name: "Lap 2" }),
      }),
    );
  });

  it("setCurrentEpochName sends null when clearing", async () => {
    const { setCurrentEpochName } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { ok: true }));
    await setCurrentEpochName("192.168.1.10", null);
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/streams/192.168.1.10/current-epoch/name",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ name: null }),
      }),
    );
  });

  it("startDownloadReads returns parsed JSON on 202", async () => {
    const { startDownloadReads } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(202, { status: "started", estimated_reads: 42 }),
    );
    const result = await startDownloadReads("192.168.1.10");
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/readers/192.168.1.10/download-reads",
      expect.objectContaining({ method: "POST" }),
    );
    expect(result).toEqual({ status: "started", estimated_reads: 42 });
  });

  it("startDownloadReads throws on 409 conflict", async () => {
    const { startDownloadReads } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(409, { error: "download already in progress" }),
    );
    await expect(startDownloadReads("192.168.1.10")).rejects.toThrow(
      "Download already in progress",
    );
  });

  it("startDownloadReads throws on other errors", async () => {
    const { startDownloadReads } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(500, "Internal Server Error"));
    await expect(startDownloadReads("192.168.1.10")).rejects.toThrow("-> 500:");
  });

  it("setReadMode sends mode and timeout", async () => {
    const { setReadMode } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { mode: "fsls" }));

    await setReadMode("192.168.1.10", "fsls", 10);

    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/readers/192.168.1.10/read-mode",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ mode: "fsls", timeout: 10 }),
      }),
    );
  });

  it("downloadUpdate throws on 409 conflict", async () => {
    const { downloadUpdate } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(409, { status: "failed", error: "no update available" }),
    );
    await expect(downloadUpdate()).rejects.toThrow(
      "Download already in progress",
    );
  });

  it("getReaderInfo fetches reader info", async () => {
    const { getReaderInfo } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { banner: "IPICO v4" }));
    const info = await getReaderInfo("192.168.1.10");
    expect(info?.banner).toBe("IPICO v4");
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/readers/192.168.1.10/info",
      expect.any(Object),
    );
  });

  it("getReaderInfo returns undefined on 204", async () => {
    const { getReaderInfo } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(204, null));
    const info = await getReaderInfo("192.168.1.10");
    expect(info).toBeUndefined();
  });

  it("syncReaderClock calls sync-clock endpoint", async () => {
    const { syncReaderClock } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, {
        reader_clock: "2026-03-07T12:00:00.000",
        clock_drift_ms: 42,
      }),
    );
    const result = await syncReaderClock("192.168.1.10");
    expect(result.clock_drift_ms).toBe(42);
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/readers/192.168.1.10/sync-clock",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("getReadMode fetches current read mode", async () => {
    const { getReadMode } = await import("./api");
    mockFetch.mockResolvedValue(
      makeResponse(200, { mode: "event", timeout: 5 }),
    );
    const result = await getReadMode("192.168.1.10");
    expect(result.mode).toBe("event");
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/readers/192.168.1.10/read-mode",
      expect.any(Object),
    );
  });

  it("getTtoState fetches current tto state", async () => {
    const { getTtoState } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { enabled: true }));
    const result = await getTtoState("192.168.1.10");
    expect(result.enabled).toBe(true);
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/readers/192.168.1.10/tto",
      expect.any(Object),
    );
  });

  it("setTtoState sends enabled flag", async () => {
    const { setTtoState } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { enabled: false }));

    const result = await setTtoState("192.168.1.10", false);

    expect(result.enabled).toBe(false);
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/readers/192.168.1.10/tto",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ enabled: false }),
      }),
    );
  });

  it("refreshReader posts to refresh endpoint", async () => {
    const { refreshReader } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { banner: "IPICO v4" }));
    await refreshReader("192.168.1.10");
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/readers/192.168.1.10/refresh",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("clearReaderRecords posts to clear-records endpoint", async () => {
    const { clearReaderRecords } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { ok: true }));
    await clearReaderRecords("192.168.1.10");
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/readers/192.168.1.10/clear-records",
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("setRecording sends enabled flag", async () => {
    const { setRecording } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { recording: true }));
    const result = await setRecording("192.168.1.10", true);
    expect(result.recording).toBe(true);
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/readers/192.168.1.10/recording",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ enabled: true }),
      }),
    );
  });

  it("reconnectReader posts to reconnect endpoint", async () => {
    const { reconnectReader } = await import("./api");
    mockFetch.mockResolvedValue(makeResponse(200, { ok: true }));
    await reconnectReader("192.168.1.10");
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/v1/readers/192.168.1.10/reconnect",
      expect.objectContaining({ method: "POST" }),
    );
  });
});
