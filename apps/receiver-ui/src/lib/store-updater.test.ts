import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  getStatus: vi.fn().mockResolvedValue({
    connection_state: "disconnected",
    local_ok: true,
    streams_count: 0,
    receiver_id: "recv-test",
  }),
  getStreams: vi.fn().mockResolvedValue({
    streams: [],
    degraded: false,
    upstream_error: null,
  }),
  getLogs: vi.fn().mockResolvedValue({ entries: [] }),
  getProfile: vi.fn().mockResolvedValue(null),
  getUpdateStatus: vi.fn().mockResolvedValue(null),
  getMode: vi.fn().mockResolvedValue({
    mode: "live",
    streams: [],
    earliest_epochs: [],
  }),
  getRaces: vi.fn().mockResolvedValue({ races: [] }),
  getReplayTargetEpochs: vi.fn().mockResolvedValue({ epochs: [] }),
  getForwarders: vi.fn().mockResolvedValue({ forwarders: [] }),
  checkForUpdate: vi.fn().mockResolvedValue({ status: "up_to_date" }),
  downloadUpdate: vi.fn().mockResolvedValue({ status: "downloaded" }),
  applyUpdate: vi.fn().mockResolvedValue(undefined),
  getDbfConfig: vi.fn().mockResolvedValue({ enabled: false, path: "" }),
  putDbfConfig: vi.fn().mockResolvedValue(undefined),
  clearDbf: vi.fn().mockResolvedValue(undefined),
  updateSubscriptionEventType: vi.fn().mockResolvedValue(undefined),
  getStreamMetrics: vi.fn().mockResolvedValue([]),
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
      server_url: "wss://receiver.example/ws",
      token: "secret-token",
      receiver_id: "recv-live",
    });

    const { initStore, store } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    expect(store.editServerUrl).toBe("wss://receiver.example/ws");
    expect(store.editToken).toBe("secret-token");
    expect(store.editReceiverId).toBe("recv-live");
    expect(store.savedServerUrl).toBe("wss://receiver.example/ws");
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
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 1,
        },
        {
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
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 7001,
          stream_epoch: 2,
        },
        {
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
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.1:10000",
      },
      "start",
    );
    expect(store.streams.streams[0]?.event_type).toBe("start");
  });

  it("maps reader control events from stream_id back to the stream key", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store, streamKey } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    store.streams = {
      streams: [
        {
          stream_id: "stream-1",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 10100,
        },
      ],
      degraded: false,
      upstream_error: null,
    } as any;

    callbacks?.onReaderInfoUpdated?.({
      stream_id: "stream-1",
      reader_ip: "10.0.0.1:10000",
      state: "connected",
      reader_info: { banner: "IPICO Reader" },
    } as any);

    callbacks?.onReaderDownloadProgress?.({
      stream_id: "stream-1",
      reader_ip: "10.0.0.1:10000",
      state: "downloading",
      reads_received: 42,
      progress: 100,
      total: 200,
      error: null,
    } as any);

    const key = streamKey("fwd-1", "10.0.0.1:10000");
    expect(store.readerInfos.get(key)).toEqual({ banner: "IPICO Reader" });
    expect(store.readerStates.get(key)).toBe("connected");
    expect(store.downloadProgress.get(key)).toEqual({
      state: "downloading",
      reads_received: 42,
      progress: 100,
      total: 200,
      error: undefined,
    });
  });
});
