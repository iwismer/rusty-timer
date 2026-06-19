import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  getStatus: vi.fn().mockResolvedValue({
    connection_state: "disconnected",
    local_ok: true,
    streams_count: 0,
    receiver_id: "recv-test",
    server: {
      configured: false,
      endpoint_id: null,
      reachable: null,
      approval_state: null,
      waiting_for_approval: false,
      message: null,
    },
  }),
  getStreams: vi.fn().mockResolvedValue({
    streams: [],
    degraded: false,
    upstream_error: null,
  }),
  getConnections: vi.fn().mockResolvedValue({
    server: {
      configured: false,
      endpoint_id: null,
      reachable: null,
      approval_state: null,
      waiting_for_approval: false,
      message: null,
    },
    forwarders: [],
  }),
  getLogs: vi.fn().mockResolvedValue({ entries: [] }),
  getProfile: vi.fn().mockResolvedValue(null),
  getUpdateStatus: vi.fn().mockResolvedValue(null),
  getMode: vi.fn().mockResolvedValue({
    mode: "live",
    streams: [],
    earliest_epochs: [],
  }),
  getReplayTargetEpochs: vi.fn().mockResolvedValue({ epochs: [] }),
  checkForUpdate: vi.fn().mockResolvedValue({ status: "up_to_date" }),
  downloadUpdate: vi.fn().mockResolvedValue({ status: "downloaded" }),
  applyUpdate: vi.fn().mockResolvedValue(undefined),
  getDbfConfig: vi.fn().mockResolvedValue({ enabled: false, path: "" }),
  putDbfConfig: vi.fn().mockResolvedValue(undefined),
  clearDbf: vi.fn().mockResolvedValue(undefined),
  updateSubscriptionEventType: vi.fn().mockResolvedValue(undefined),
  getStreamMetrics: vi.fn().mockResolvedValue([]),
  reconnectServer: vi.fn().mockResolvedValue(undefined),
}));

const desktopUpdaterMocks = vi.hoisted(() => ({
  loadDesktopVersion: vi.fn().mockResolvedValue({
    supported: true,
    version: "0.8.0",
  }),
  checkForDesktopUpdate: vi.fn().mockResolvedValue({
    supported: true,
    update: {
      currentVersion: "0.8.0",
      version: "0.9.0",
      notes: "Receiver release notes",
      publishedAt: "2026-03-20T10:00:00Z",
    },
  }),
  installDesktopUpdate: vi.fn().mockResolvedValue(undefined),
}));

const eventMocks = vi.hoisted(() => {
  const listeners = new Map<string, () => void>();
  return {
    listeners,
    listen: vi.fn(
      async (eventName: string, callback: () => void): Promise<() => void> => {
        listeners.set(eventName, callback);
        return () => {
          listeners.delete(eventName);
        };
      },
    ),
  };
});

const sseMocks = vi.hoisted(() => ({
  initSSE: vi.fn(),
  destroySSE: vi.fn(),
}));

const darkModeMocks = vi.hoisted(() => ({
  cycleTheme: vi.fn(),
}));

const mockFetch = vi.hoisted(() => vi.fn());

vi.mock("./api", () => apiMocks);
vi.mock("./desktop-updater", () => desktopUpdaterMocks);
vi.mock("./sse", () => sseMocks);
vi.mock("@tauri-apps/api/event", () => ({
  listen: eventMocks.listen,
}));
vi.mock("@rusty-timer/shared-ui/lib/dark-mode", () => darkModeMocks);

vi.stubGlobal("fetch", mockFetch);

async function flushAsyncWork(): Promise<void> {
  await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

function mockSseInitWithCallbacks(): {
  callbacks: Parameters<typeof sseMocks.initSSE>[0] | undefined;
} {
  const state: {
    callbacks: Parameters<typeof sseMocks.initSSE>[0] | undefined;
  } = { callbacks: undefined };
  sseMocks.initSSE.mockImplementation((callbacks) => {
    state.callbacks = callbacks;
    return Promise.resolve();
  });
  return state;
}

describe("receiver updater store", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
    eventMocks.listeners.clear();
    mockFetch.mockResolvedValue({
      json: async () => ({ version: "legacy-version" }),
    });
  });

  it("loads the app version from the desktop updater instead of the receiver version endpoint", async () => {
    const { initStore, store } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    expect(desktopUpdaterMocks.loadDesktopVersion).toHaveBeenCalledTimes(1);
    expect(store.appVersion).toBe("0.8.0");
    expect(mockFetch).not.toHaveBeenCalled();
  });

  it("checks for updates through Tauri when the menu event fires and opens the modal", async () => {
    const { initStore, store } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const onCheckUpdate = eventMocks.listeners.get("menu-check-update");
    expect(onCheckUpdate).toBeTypeOf("function");
    desktopUpdaterMocks.checkForDesktopUpdate.mockClear();

    onCheckUpdate?.();
    await flushAsyncWork();

    expect(desktopUpdaterMocks.checkForDesktopUpdate).toHaveBeenCalledTimes(1);
    expect(apiMocks.checkForUpdate).not.toHaveBeenCalled();
    expect(store.updateModalOpen).toBe(true);
    expect(store.updateState?.notes).toBe("Receiver release notes");
  });

  it("installs through the desktop updater instead of receiver download/apply endpoints", async () => {
    const { confirmUpdateInstall, store } = await import("./store.svelte");

    store.updateState = {
      status: "available",
      currentVersion: "0.8.0",
      version: "0.9.0",
      notes: null,
      busy: false,
      error: null,
    };

    await confirmUpdateInstall();

    expect(desktopUpdaterMocks.installDesktopUpdate).toHaveBeenCalledTimes(1);
    expect(apiMocks.downloadUpdate).not.toHaveBeenCalled();
    expect(apiMocks.applyUpdate).not.toHaveBeenCalled();
  });

  it("hydrates config edit fields from the saved profile on initial load", async () => {
    apiMocks.getProfile.mockResolvedValueOnce({
      server_url: "https://receiver.example",
      token: "secret-token",
      receiver_id: "recv-live",
    });

    const { initStore, store } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    expect(store.editServerUrl).toBe("https://receiver.example");
    expect(store.editToken).toBe("secret-token");
    expect(store.editReceiverId).toBe("recv-live");
    expect(store.savedServerUrl).toBe("https://receiver.example");
    expect(store.savedToken).toBe("secret-token");
    expect(store.savedReceiverId).toBe("recv-live");
  });

  it("resets hydrated mode to default live mode when no mode is configured", async () => {
    apiMocks.getMode.mockResolvedValueOnce({
      mode: "race",
      race_id: "11111111-1111-1111-1111-111111111111",
    });

    const { initStore, loadAll, store } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    expect(store.modeDraft).toBe("race");
    expect(store.raceIdDraft).toBe("11111111-1111-1111-1111-111111111111");

    apiMocks.getMode.mockRejectedValueOnce(new Error("no mode configured"));

    await loadAll();

    expect(store.modeDraft).toBe("live");
    expect(store.raceIdDraft).toBe("");
    expect(store.targetedEpochInputs).toEqual({});
    expect(store.savedModePayload).toBe(
      JSON.stringify({ mode: "live", streams: [], earliest_epochs: [] }),
    );
  });

  it("force load resets dirty mode state after clear data removes persisted mode", async () => {
    apiMocks.getMode.mockResolvedValueOnce({
      mode: "race",
      race_id: "11111111-1111-1111-1111-111111111111",
    });

    const { initStore, loadAll, markModeEdited, store } =
      await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    store.modeDraft = "targeted_replay";
    store.targetedEpochInputs = {
      "fwd-1/10.0.0.1:10000": "12",
    };
    markModeEdited();

    apiMocks.getMode.mockRejectedValueOnce(new Error("no mode configured"));

    await loadAll({ forceHydrateMode: true });

    expect(store.modeDraft).toBe("live");
    expect(store.raceIdDraft).toBe("");
    expect(store.targetedEpochInputs).toEqual({});
    expect(store.savedModePayload).toBe(
      JSON.stringify({ mode: "live", streams: [], earliest_epochs: [] }),
    );
  });

  it("preserves server status across incremental status events", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store } = await import("./store.svelte");

    apiMocks.getStatus.mockResolvedValueOnce({
      connection_state: "connecting",
      local_ok: true,
      streams_count: 0,
      receiver_id: "recv-test",
      server: {
        configured: true,
        endpoint_id: "node-1",
        reachable: true,
        approval_state: "pending",
        waiting_for_approval: true,
        message: "Waiting for server admin approval",
      },
    });

    initStore();
    await flushAsyncWork();

    sseState.callbacks?.onStatusChanged({
      connection_state: "connected",
      local_ok: true,
      streams_count: 1,
      receiver_id: "recv-test",
    });

    expect(store.status?.connection_state).toBe("connected");
    expect(store.status?.server.waiting_for_approval).toBe(true);
  });

  it("clears cached metrics for a stream when a snapshot reports a newer epoch", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store, streamKey } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamKey("fwd-1", "10.0.0.1:10000");
    store.streamMetrics = new Map([
      [
        key,
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          raw_count: 10,
          dedup_count: 9,
          retransmit_count: 1,
          lag_ms: 1000,
          epoch_raw_count: 4,
          epoch_dedup_count: 3,
          epoch_retransmit_count: 1,
          unique_chips: 2,
          epoch_last_received_at: "2026-03-21T12:00:00Z",
          epoch_lag_ms: 250,
        },
      ],
    ]);
    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 1,
        },
      ],
      degraded: false,
      upstream_error: null,
    };

    callbacks?.onStreamsSnapshot({
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 2,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    expect(store.streamMetrics.has(key)).toBe(false);
  });

  it("preserves cached metrics for a stream when the snapshot keeps the same epoch", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store, streamKey } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamKey("fwd-1", "10.0.0.1:10000");
    const metrics = {
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
      raw_count: 10,
      dedup_count: 9,
      retransmit_count: 1,
      lag_ms: 1000,
      epoch_raw_count: 4,
      epoch_dedup_count: 3,
      epoch_retransmit_count: 1,
      unique_chips: 2,
      epoch_last_received_at: "2026-03-21T12:00:00Z",
      epoch_lag_ms: 250,
    };
    store.streamMetrics = new Map([[key, metrics]]);
    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 2,
        },
      ],
      degraded: false,
      upstream_error: null,
    };

    callbacks?.onStreamsSnapshot({
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 2,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    expect(store.streamMetrics.get(key)).toEqual(metrics);
  });

  it("onStreamsSnapshot keeps metrics for newly appearing streams", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store, streamKey } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    // Pre-populate metrics for a stream (simulates metrics arriving before snapshot)
    const key = streamKey("fwd-new", "10.0.0.1:10000");
    const metrics = {
      forwarder_id: "fwd-new",
      reader_ip: "10.0.0.1:10000",
      raw_count: 100,
      dedup_count: 90,
      retransmit_count: 10,
      lag_ms: null,
      epoch_raw_count: 50,
      epoch_dedup_count: 45,
      epoch_retransmit_count: 5,
      epoch_lag_ms: null,
      epoch_last_received_at: null,
      unique_chips: 20,
    };
    store.streamMetrics = new Map([[key, metrics]]);
    // No previous streams (simulates first snapshot or stream re-appearing)
    store.streams = null;

    callbacks?.onStreamsSnapshot({
      streams: [
        {
          forwarder_id: "fwd-new",
          reader_ip: "10.0.0.1:10000",
          stream_epoch: undefined,
        } as any,
      ],
      degraded: false,
      upstream_error: null,
    });

    // Metrics should be preserved — only prune on known epoch changes
    expect(store.streamMetrics.get(key)).toEqual(metrics);
  });

  it("onStreamsSnapshot keeps metrics when previous epoch was undefined", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store, streamKey } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamKey("fwd-1", "10.0.0.1:10000");
    const metrics = {
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
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
    store.streamMetrics = new Map([[key, metrics]]);
    // Previous streams had undefined epoch (local-only data during reconnect)
    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: undefined,
        } as any,
      ],
      degraded: true,
      upstream_error: "connection state: Connecting",
    };

    // New snapshot arrives with real epoch after reconnect
    callbacks?.onStreamsSnapshot({
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 1,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    // Metrics should survive — undefined→real is not a real epoch change
    expect(store.streamMetrics.get(key)).toEqual(metrics);
  });

  it("onStreamsSnapshot clears metrics after reconnect when the concrete epoch changed", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store, streamKey } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamKey("fwd-1", "10.0.0.1:10000");
    const metrics = {
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
      raw_count: 12,
      dedup_count: 11,
      retransmit_count: 1,
      lag_ms: 500,
      epoch_raw_count: 7,
      epoch_dedup_count: 6,
      epoch_retransmit_count: 1,
      epoch_lag_ms: 200,
      epoch_last_received_at: "2026-03-21T12:00:00Z",
      unique_chips: 4,
    };
    store.streamMetrics = new Map([[key, metrics]]);
    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 1,
        },
      ],
      degraded: false,
      upstream_error: null,
    };

    callbacks?.onStreamsSnapshot({
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: undefined,
        } as any,
      ],
      degraded: true,
      upstream_error: "connection state: Connecting",
    });

    expect(store.streamMetrics.get(key)).toEqual(metrics);

    callbacks?.onStreamsSnapshot({
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 2,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    expect(store.streamMetrics.has(key)).toBe(false);
  });

  it("onStreamsSnapshot keeps metrics through multiple consecutive undefined-epoch snapshots", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store, streamKey } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamKey("fwd-1", "10.0.0.1:10000");
    const metrics = {
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
      raw_count: 5,
      dedup_count: 5,
      retransmit_count: 0,
      lag_ms: null,
      epoch_raw_count: 5,
      epoch_dedup_count: 5,
      epoch_retransmit_count: 0,
      epoch_lag_ms: null,
      epoch_last_received_at: null,
      unique_chips: 3,
    };
    store.streamMetrics = new Map([[key, metrics]]);
    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 1,
        },
      ],
      degraded: false,
      upstream_error: null,
    };

    // First undefined snapshot (disconnect)
    callbacks?.onStreamsSnapshot({
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          stream_epoch: undefined,
        } as any,
      ],
      degraded: true,
      upstream_error: "connection state: Connecting",
    });
    expect(store.streamMetrics.get(key)).toEqual(metrics);

    // Second consecutive undefined snapshot (still disconnected)
    callbacks?.onStreamsSnapshot({
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          stream_epoch: undefined,
        } as any,
      ],
      degraded: true,
      upstream_error: "connection state: Connecting",
    });
    expect(store.streamMetrics.get(key)).toEqual(metrics);

    // Reconnect with new epoch — should clear
    callbacks?.onStreamsSnapshot({
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 2,
        },
      ],
      degraded: false,
      upstream_error: null,
    });
    expect(store.streamMetrics.has(key)).toBe(false);
  });

  it("onStreamsSnapshot clears metrics for stream that disappears and reappears with new epoch", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store, streamKey } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamKey("fwd-1", "10.0.0.1:10000");
    const metrics = {
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
      raw_count: 10,
      dedup_count: 9,
      retransmit_count: 1,
      lag_ms: null,
      epoch_raw_count: 10,
      epoch_dedup_count: 9,
      epoch_retransmit_count: 1,
      epoch_lag_ms: null,
      epoch_last_received_at: null,
      unique_chips: 5,
    };
    store.streamMetrics = new Map([[key, metrics]]);
    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 1,
        },
      ],
      degraded: false,
      upstream_error: null,
    };

    // Stream disappears entirely
    callbacks?.onStreamsSnapshot({
      streams: [],
      degraded: false,
      upstream_error: null,
    });
    // Metrics pruned because stream is no longer in snapshot
    expect(store.streamMetrics.has(key)).toBe(false);
  });

  it("onStreamsSnapshot handles null epoch same as undefined", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store, streamKey } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamKey("fwd-1", "10.0.0.1:10000");
    const metrics = {
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
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
    store.streamMetrics = new Map([[key, metrics]]);
    store.streams = null;

    // null epoch should behave identically to undefined — metrics preserved
    callbacks?.onStreamsSnapshot({
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          stream_epoch: null,
        } as any,
      ],
      degraded: false,
      upstream_error: null,
    });

    expect(store.streamMetrics.get(key)).toEqual(metrics);
  });

  it("onStreamsSnapshot prunes only the stream whose epoch changed in a multi-stream snapshot", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store, streamKey } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const keyA = streamKey("fwd-1", "10.0.0.1:10000");
    const keyB = streamKey("fwd-2", "10.0.0.2:10000");
    const metricsA = {
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
      raw_count: 5,
      dedup_count: 5,
      retransmit_count: 0,
      lag_ms: null,
      epoch_raw_count: 5,
      epoch_dedup_count: 5,
      epoch_retransmit_count: 0,
      epoch_lag_ms: null,
      epoch_last_received_at: null,
      unique_chips: 3,
    };
    const metricsB = {
      forwarder_id: "fwd-2",
      reader_ip: "10.0.0.2:10000",
      raw_count: 20,
      dedup_count: 18,
      retransmit_count: 2,
      lag_ms: null,
      epoch_raw_count: 20,
      epoch_dedup_count: 18,
      epoch_retransmit_count: 2,
      epoch_lag_ms: null,
      epoch_last_received_at: null,
      unique_chips: 10,
    };
    store.streamMetrics = new Map([
      [keyA, metricsA],
      [keyB, metricsB],
    ]);
    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 1,
        },
        {
          forwarder_endpoint_id: "fwd-2",
          stream_id: "stream-10.0.0.2:10000",
          forwarder_id: "fwd-2",
          reader_ip: "10.0.0.2:10000",
          subscribed: true,
          local_port: 7002,
          stream_epoch: 3,
        },
      ],
      degraded: false,
      upstream_error: null,
    };

    // Stream A epoch changes, Stream B stays the same
    callbacks?.onStreamsSnapshot({
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 2,
        },
        {
          forwarder_endpoint_id: "fwd-2",
          stream_id: "stream-10.0.0.2:10000",
          forwarder_id: "fwd-2",
          reader_ip: "10.0.0.2:10000",
          subscribed: true,
          local_port: 7002,
          stream_epoch: 3,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    // Stream A metrics pruned (epoch changed), Stream B preserved
    expect(store.streamMetrics.has(keyA)).toBe(false);
    expect(store.streamMetrics.get(keyB)).toEqual(metricsB);
  });

  it("keeps cached metrics across resync until replacement data arrives", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store, streamKey } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamKey("fwd-1", "10.0.0.1:10000");
    const metrics = {
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
      raw_count: 10,
      dedup_count: 9,
      retransmit_count: 1,
      lag_ms: 1000,
      epoch_raw_count: 4,
      epoch_dedup_count: 3,
      epoch_retransmit_count: 1,
      unique_chips: 2,
      epoch_last_received_at: "2026-03-21T12:00:00Z",
      epoch_lag_ms: 250,
    };
    store.streamMetrics = new Map([[key, metrics]]);

    callbacks?.onResync();
    await flushAsyncWork();

    expect(store.streamMetrics.get(key)).toEqual(metrics);
  });

  it("updates the stream DBF event type through the API and local store", async () => {
    const { store, updateStreamEventType } = await import("./store.svelte");

    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 10100,
        },
      ],
      degraded: false,
      upstream_error: null,
    };

    await updateStreamEventType(store.streams.streams[0], "start");

    expect(apiMocks.updateSubscriptionEventType).toHaveBeenCalledWith(
      {
        forwarder_endpoint_id: "fwd-1",
        stream_id: "stream-10.0.0.1:10000",
      },
      "start",
    );
    expect(store.streams.streams[0]?.event_type).toBe("start");
  });

  it("does not block one canonical-only stream event-type update with another", async () => {
    const { store, updateStreamEventType } = await import("./store.svelte");

    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "endpoint-1",
          stream_id: "stream-1",
          subscribed: true,
          local_port: 10100,
        },
        {
          forwarder_endpoint_id: "endpoint-2",
          stream_id: "stream-2",
          subscribed: true,
          local_port: 10200,
        },
      ],
      degraded: false,
      upstream_error: null,
    };

    let resolveFirst!: () => void;
    apiMocks.updateSubscriptionEventType
      .mockImplementationOnce(
        () =>
          new Promise<void>((resolve) => {
            resolveFirst = resolve;
          }),
      )
      .mockResolvedValueOnce(undefined);

    const firstUpdate = updateStreamEventType(
      store.streams.streams[0],
      "start",
    );
    await flushAsyncWork();

    await updateStreamEventType(store.streams.streams[1], "finish");

    expect(apiMocks.updateSubscriptionEventType).toHaveBeenCalledTimes(2);
    expect(apiMocks.updateSubscriptionEventType).toHaveBeenNthCalledWith(
      1,
      { forwarder_endpoint_id: "endpoint-1", stream_id: "stream-1" },
      "start",
    );
    expect(apiMocks.updateSubscriptionEventType).toHaveBeenNthCalledWith(
      2,
      { forwarder_endpoint_id: "endpoint-2", stream_id: "stream-2" },
      "finish",
    );

    resolveFirst();
    await firstUpdate;
  });

  it("keeps configured-but-unavailable UPS entries when no sampled status has arrived yet", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    sseState.callbacks?.onForwarderUpsUpdated?.({
      forwarder_id: "fwd-1",
      available: false,
      status: null,
    });

    expect(store.upsState.get("fwd-1")).toEqual({
      available: false,
      status: null,
    });
  });

  it("keeps UPS entries because refresh no longer loads a central forwarder list", async () => {
    const { initStore, loadAll, store } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    store.upsState = new Map([
      [
        "fwd-1",
        {
          available: true,
          status: {
            battery_percent: 80,
            battery_voltage_mv: 4010,
            charging: false,
            power_plugged: true,
            temperature_cdeg: 2500,
            sampled_at: 1711929600000,
          },
        },
      ],
      [
        "fwd-2",
        {
          available: false,
          status: null,
        },
      ],
    ]);

    await loadAll();

    expect(Array.from(store.upsState.keys())).toEqual(["fwd-1", "fwd-2"]);
  });
});

describe("canonical-only stream identity", () => {
  it("gives two canonical-only streams distinct identities and per-stream epoch state", async () => {
    const { store, streamIdentity, selectedEarliestEpochValue } =
      await import("./store.svelte");

    const streamA = {
      forwarder_endpoint_id: "endpoint-1",
      stream_id: "11111111-1111-1111-1111-111111111111",
      subscribed: true,
      local_port: null,
      stream_epoch: 5,
    };
    const streamB = {
      forwarder_endpoint_id: "endpoint-2",
      stream_id: "22222222-2222-2222-2222-222222222222",
      subscribed: true,
      local_port: null,
      stream_epoch: 9,
    };
    store.streams = {
      streams: [streamA, streamB],
      degraded: false,
      upstream_error: null,
    };

    // Distinct canonical identities even though both lack legacy metadata
    // (legacy streamKey would collapse both to "/").
    expect(streamIdentity(streamA)).not.toBe(streamIdentity(streamB));

    store.earliestEpochOptions = {
      [streamIdentity(streamA)]: [
        { stream_epoch: 5, name: null, first_seen_at: null, race_names: [] },
      ],
      [streamIdentity(streamB)]: [
        { stream_epoch: 9, name: null, first_seen_at: null, race_names: [] },
      ],
    };
    store.earliestEpochInputs = {
      [streamIdentity(streamA)]: "5",
      [streamIdentity(streamB)]: "9",
    };

    expect(selectedEarliestEpochValue(streamA)).toBe("5");
    expect(selectedEarliestEpochValue(streamB)).toBe("9");
  });

  it("uses advertised stream epochs for canonical-only epoch options", async () => {
    const { store, streamIdentity, prefetchEarliestEpochOptions } =
      await import("./store.svelte");

    store.earliestEpochOptions = {};
    store.earliestEpochLoading = {};

    const stream = {
      forwarder_endpoint_id: "endpoint-1",
      stream_id: "11111111-1111-1111-1111-111111111111",
      subscribed: true,
      local_port: null,
      stream_epoch: 12,
      current_epoch_name: "Race Morning",
    };

    await prefetchEarliestEpochOptions([stream]);

    expect(store.earliestEpochOptions[streamIdentity(stream)]).toEqual([
      {
        stream_epoch: 12,
        name: "Race Morning",
        first_seen_at: null,
        race_names: [],
      },
    ]);
    expect(apiMocks.getReplayTargetEpochs).not.toHaveBeenCalled();
  });

  it("excludes canonical-only streams from the legacy live mode payload but keeps legacy ones", async () => {
    const { store, streamIdentity, modePayload } =
      await import("./store.svelte");

    const legacyStream = {
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
      subscribed: true,
      local_port: 10100,
      stream_epoch: 3,
    };
    const canonicalOnly = {
      forwarder_endpoint_id: "endpoint-2",
      stream_id: "22222222-2222-2222-2222-222222222222",
      subscribed: true,
      local_port: null,
      stream_epoch: 7,
    };
    store.modeDraft = "live";
    store.streams = {
      streams: [legacyStream, canonicalOnly],
      degraded: false,
      upstream_error: null,
    };
    store.earliestEpochInputs = {
      [streamIdentity(legacyStream)]: "3",
      [streamIdentity(canonicalOnly)]: "7",
    };

    const payload = modePayload();
    expect(payload.mode).toBe("live");
    if (payload.mode !== "live") throw new Error("unreachable");
    // Only the stream with display metadata is representable in the compatibility payload.
    expect(payload.earliest_epochs).toEqual([
      { forwarder_id: "fwd-1", reader_ip: "10.0.0.1:10000", earliest_epoch: 3 },
    ]);
  });
});
