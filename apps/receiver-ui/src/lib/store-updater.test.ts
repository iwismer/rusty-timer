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
  getStreamEpochs: vi.fn().mockResolvedValue({ epochs: [] }),
  checkForUpdate: vi.fn().mockResolvedValue({ status: "up_to_date" }),
  downloadUpdate: vi.fn().mockResolvedValue({ status: "downloaded" }),
  applyUpdate: vi.fn().mockResolvedValue(undefined),
  getDbfConfig: vi.fn().mockResolvedValue({ enabled: false }),
  putDbfConfig: vi.fn().mockResolvedValue(undefined),
  clearDbf: vi.fn().mockResolvedValue(undefined),
  getRdImportConfig: vi.fn().mockResolvedValue({
    enabled: false,
    dir: "C:\\Winrace\\Files",
    interval_secs: 15,
  }),
  putRdImportConfig: vi.fn().mockResolvedValue(undefined),
  getDataStats: vi.fn().mockResolvedValue({
    participants: 0,
    chips: 0,
    matched_participants: 0,
    participants_without_chips: 0,
    resolvable_chips: 0,
  }),
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

function testStream(overrides: Record<string, unknown> = {}) {
  return {
    forwarder_endpoint_id: "endpoint-a",
    stream_id: "stream-a",
    forwarder_id: "fwd-1",
    reader_ip: "10.0.0.1:10000",
    subscribed: true,
    online: false,
    local_port: 7000,
    ...overrides,
  };
}

function testMetrics(overrides: Record<string, unknown> = {}) {
  return {
    forwarder_endpoint_id: "endpoint-a",
    stream_id: "stream-a",
    forwarder_id: "fwd-1",
    reader_ip: "10.0.0.1:10000",
    raw_count: 1,
    dedup_count: 1,
    retransmit_count: 0,
    lag_ms: null,
    epoch_raw_count: 1,
    epoch_dedup_count: 1,
    epoch_retransmit_count: 0,
    unique_chips: 1,
    epoch_last_received_at: null,
    epoch_lag_ms: null,
    ...overrides,
  };
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

  it("expires stream activity only after the recency window", async () => {
    const { store, streamHasRecentActivity, streamIdentity } =
      await import("./store.svelte");
    const stream = testStream();
    const now = 50_000;
    store.streamActivityAt = new Map([[streamIdentity(stream), now - 10_000]]);

    expect(streamHasRecentActivity(stream, now)).toBe(true);
    expect(streamHasRecentActivity(stream, now + 1)).toBe(false);
  });

  it("expires optimistic subscribe only after the grace window", async () => {
    const { store, streamIdentity, streamIsOptimisticallySubscribing } =
      await import("./store.svelte");
    const stream = testStream({ subscribed: true, online: false });
    const now = 50_000;
    store.streamSubscriptionPendingSince = {
      [streamIdentity(stream)]: now - 10_000,
    };

    expect(streamIsOptimisticallySubscribing(stream, now)).toBe(true);
    expect(streamIsOptimisticallySubscribing(stream, now + 1)).toBe(false);
  });

  it("does not show optimistic subscribe for unsubscribed streams", async () => {
    const { store, streamIdentity, streamIsOptimisticallySubscribing } =
      await import("./store.svelte");
    const stream = testStream({ subscribed: false, online: false });
    store.streamSubscriptionPendingSince = { [streamIdentity(stream)]: 1_000 };

    expect(streamIsOptimisticallySubscribing(stream, 1_001)).toBe(false);
  });

  it("does not show optimistic subscribe for online streams", async () => {
    const { store, streamIdentity, streamIsOptimisticallySubscribing } =
      await import("./store.svelte");
    const stream = testStream({ subscribed: true, online: true });
    store.streamSubscriptionPendingSince = { [streamIdentity(stream)]: 1_000 };

    expect(streamIsOptimisticallySubscribing(stream, 1_001)).toBe(false);
  });

  it("suppresses optimistic subscribe when the stream has recent activity", async () => {
    const { store, streamIdentity, streamIsOptimisticallySubscribing } =
      await import("./store.svelte");
    const stream = testStream({ subscribed: true, online: false });
    const now = 50_000;
    store.streamSubscriptionPendingSince = { [streamIdentity(stream)]: now };
    store.streamActivityAt = new Map([[streamIdentity(stream), now]]);

    expect(streamIsOptimisticallySubscribing(stream, now)).toBe(false);
  });

  it("keys delta activity by canonical stream identity instead of legacy display key", async () => {
    const sseState = mockSseInitWithCallbacks();
    const {
      initStore,
      store,
      streamHasRecentActivity,
      streamIdentity,
      streamKey,
    } = await import("./store.svelte");
    const now = 50_000;
    const dateNow = vi.spyOn(Date, "now").mockReturnValue(now);
    try {
      const streamA = testStream({
        forwarder_endpoint_id: "endpoint-a",
        stream_id: "stream-a",
      });
      const streamB = testStream({
        forwarder_endpoint_id: "endpoint-b",
        stream_id: "stream-b",
      });
      store.streams = {
        streams: [streamA, streamB],
        degraded: false,
        upstream_error: null,
      };

      initStore();
      sseState.callbacks?.onStreamDeltas([
        {
          forwarder_endpoint_id: "endpoint-a",
          stream_id: "stream-a",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          reads_total: 1,
          reads_epoch: 1,
          metrics: testMetrics(),
          last_read: null,
        },
      ]);

      expect(store.streamActivityAt.has(streamIdentity(streamA))).toBe(true);
      expect(
        store.streamActivityAt.has(streamKey("fwd-1", "10.0.0.1:10000")),
      ).toBe(false);
      expect(streamHasRecentActivity(streamA, now)).toBe(true);
      expect(streamHasRecentActivity(streamB, now)).toBe(false);

      // Read counts stay isolated: only the delta's own stream is patched,
      // never a sibling that shares legacy (forwarder_id, reader_ip).
      const rows = store.streams?.streams ?? [];
      const rowA = rows.find(
        (s) => streamIdentity(s) === streamIdentity(streamA),
      );
      const rowB = rows.find(
        (s) => streamIdentity(s) === streamIdentity(streamB),
      );
      expect(rowA?.reads_total).toBe(1);
      expect(rowA?.reads_epoch).toBe(1);
      expect(rowB?.reads_total).toBeUndefined();
      expect(rowB?.reads_epoch).toBeUndefined();

      // Metrics are keyed by canonical identity, not the shared display key.
      expect(store.streamMetrics.has(streamIdentity(streamA))).toBe(true);
      expect(store.streamMetrics.has(streamIdentity(streamB))).toBe(false);
      expect(
        store.streamMetrics.has(streamKey("fwd-1", "10.0.0.1:10000")),
      ).toBe(false);
    } finally {
      dateNow.mockRestore();
    }
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

  it("hydrates Race Director import config on initial load", async () => {
    apiMocks.getRdImportConfig.mockResolvedValueOnce({
      enabled: true,
      dir: "D:\\Race\\Files",
      interval_secs: 30,
    });

    const { initStore, store } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    expect(store.rdImportEnabled).toBe(true);
    expect(store.rdImportDir).toBe("D:\\Race\\Files");
    expect(store.rdImportIntervalSecs).toBe(30);
    expect(store.editRdImportEnabled).toBe(true);
    expect(store.editRdImportDir).toBe("D:\\Race\\Files");
    expect(store.editRdImportIntervalSecs).toBe(30);
  });

  it("hydrates participant and chip data stats on initial load", async () => {
    apiMocks.getDataStats.mockResolvedValueOnce({
      participants: 120,
      chips: 118,
      matched_participants: 117,
      participants_without_chips: 3,
      resolvable_chips: 117,
    });

    const { initStore, store } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    expect(store.dataStats).toEqual({
      participants: 120,
      chips: 118,
      matched_participants: 117,
      participants_without_chips: 3,
      resolvable_chips: 117,
    });
  });

  it("keeps loading status, streams, and logs when connections fail to load", async () => {
    const { loadAll, store } = await import("./store.svelte");
    const previousConnections = {
      server: {
        configured: true,
        endpoint_id: "server-node-1",
        reachable: true,
        approval_state: "active",
        waiting_for_approval: false,
        message: null,
      },
      forwarders: [],
    };
    store.connections = previousConnections;
    apiMocks.getConnections.mockRejectedValueOnce(
      new Error("connections down"),
    );
    apiMocks.getStatus.mockResolvedValueOnce({
      connection_state: "connected",
      local_ok: true,
      streams_count: 1,
      receiver_id: "recv-after-connections-failure",
      server: previousConnections.server,
    });
    apiMocks.getStreams.mockResolvedValueOnce({
      streams: [],
      degraded: true,
      upstream_error: "stream warning",
    });
    apiMocks.getLogs.mockResolvedValueOnce({ entries: ["loaded logs"] });

    await loadAll();

    expect(store.error).toBeNull();
    expect(store.status?.receiver_id).toBe("recv-after-connections-failure");
    expect(store.streams?.degraded).toBe(true);
    expect(store.logEntries).toEqual(["loaded logs"]);
    expect(store.connections).toEqual(previousConnections);
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
    expect(store.savedModePayload).toBe(
      JSON.stringify({ mode: "live", streams: [] }),
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

    store.modeDraft = "race";
    store.raceIdDraft = "22222222-2222-2222-2222-222222222222";
    markModeEdited();

    apiMocks.getMode.mockRejectedValueOnce(new Error("no mode configured"));

    await loadAll({ forceHydrateMode: true });

    expect(store.modeDraft).toBe("live");
    expect(store.raceIdDraft).toBe("");
    expect(store.savedModePayload).toBe(
      JSON.stringify({ mode: "live", streams: [] }),
    );
  });

  it("patches reader counts in place on forwarder_reader_counts_updated without reloading connections", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const reader = (stream_id: string) => ({
      stream_id,
      connected: true,
      state: "online",
      last_read_unix_ms: 1000,
      reads_session: 1,
      reads_total: 10,
      last_seen_secs: 60,
      current_epoch_name: null,
      hardware_reader_id: null,
      firmware_version: null,
      model: null,
      reader_info: null,
      download_progress: null,
      local_port: null,
    });
    store.connections = {
      server: {
        configured: true,
        endpoint_id: "server-node-1",
        reachable: true,
        approval_state: "active",
        waiting_for_approval: false,
        message: null,
      },
      forwarders: [
        {
          endpoint_id: "endpoint-a",
          display_name: "Finish Line",
          state: "connected",
          pending: false,
          subscribed_count: 1,
          available_count: 2,
          readers: [reader("10.0.0.1:10000"), reader("10.0.0.2:10000")],
          ups: null,
          restart_needed: null,
          remote_config_available: false,
        },
      ],
    };
    apiMocks.getConnections.mockClear();

    sseState.callbacks?.onForwarderReaderCountsUpdated({
      forwarder_id: "endpoint-a",
      stream_id: "10.0.0.2:10000",
      reads_session: 7,
      reads_total: 42,
      last_read_unix_ms: 2000,
      last_seen_secs: 1,
    });

    const readers = store.connections?.forwarders[0]?.readers ?? [];
    expect(readers[1]).toMatchObject({
      reads_session: 7,
      reads_total: 42,
      last_read_unix_ms: 2000,
      last_seen_secs: 1,
    });
    // The other reader and structural fields are untouched, and no full
    // connections reload is triggered.
    expect(readers[0]).toMatchObject({ reads_session: 1, reads_total: 10 });
    expect(readers[1]?.connected).toBe(true);
    expect(apiMocks.getConnections).not.toHaveBeenCalled();

    // Updates for unknown forwarders/readers are ignored (structural changes
    // still arrive via connections_changed).
    const before = store.connections;
    sseState.callbacks?.onForwarderReaderCountsUpdated({
      forwarder_id: "endpoint-unknown",
      stream_id: "10.0.0.1:10000",
      reads_session: 99,
      reads_total: 999,
      last_read_unix_ms: null,
      last_seen_secs: null,
    });
    expect(store.connections).toBe(before);
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
    const { initStore, store, streamIdentity } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamIdentity({
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
    });
    store.streamMetrics = new Map([
      [
        key,
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
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
    const { initStore, store, streamIdentity } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamIdentity({
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
    });
    const metrics = {
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
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

  it("keeps cached epoch dropdown options when a stream snapshot only updates reads", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store, streamIdentity } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const previousStream = {
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
      subscribed: true,
      local_port: 7001,
      stream_epoch: 2,
      reads_total: 10,
    };
    const key = streamIdentity(previousStream);
    const options = [
      {
        stream_epoch: 2,
        name: "Finish",
        first_seen_at: null,
        selectable: true,
      },
      { stream_epoch: 1, name: "Start", first_seen_at: null, selectable: true },
    ];
    store.streams = {
      streams: [previousStream],
      degraded: false,
      upstream_error: null,
    };
    store.streamEpochOptions = { [key]: options };
    store.earliestEpochLoading = {};
    apiMocks.getStreamEpochs.mockClear();

    callbacks?.onStreamsSnapshot({
      streams: [
        {
          ...previousStream,
          reads_total: 11,
        },
      ],
      degraded: false,
      upstream_error: null,
    });
    await flushAsyncWork();

    expect(apiMocks.getStreamEpochs).not.toHaveBeenCalled();
    expect(store.streamEpochOptions[key]).toEqual(options);
    expect(store.earliestEpochLoading[key]).toBeUndefined();
  });

  it("onStreamsSnapshot keeps metrics for newly appearing streams", async () => {
    const sseState = mockSseInitWithCallbacks();
    const { initStore, store, streamIdentity } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    // Pre-populate metrics for a stream (simulates metrics arriving before snapshot)
    const key = streamIdentity({
      forwarder_endpoint_id: "fwd-new",
      stream_id: "stream-10.0.0.1:10000",
    });
    const metrics = {
      forwarder_endpoint_id: "fwd-new",
      stream_id: "stream-10.0.0.1:10000",
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
          forwarder_endpoint_id: "fwd-new",
          stream_id: "stream-10.0.0.1:10000",
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
    const { initStore, store, streamIdentity } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamIdentity({
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
    });
    const metrics = {
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
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
    const { initStore, store, streamIdentity } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamIdentity({
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
    });
    const metrics = {
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
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
    const { initStore, store, streamIdentity } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamIdentity({
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
    });
    const metrics = {
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
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
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
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
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
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
    const { initStore, store, streamIdentity } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamIdentity({
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
    });
    const metrics = {
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
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
    const { initStore, store, streamIdentity } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamIdentity({
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
    });
    const metrics = {
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
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
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
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
    const { initStore, store, streamIdentity } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const keyA = streamIdentity({
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
    });
    const keyB = streamIdentity({
      forwarder_endpoint_id: "fwd-2",
      stream_id: "stream-10.0.0.2:10000",
    });
    const metricsA = {
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
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
      forwarder_endpoint_id: "fwd-2",
      stream_id: "stream-10.0.0.2:10000",
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
    const { initStore, store, streamIdentity } = await import("./store.svelte");

    initStore();
    await flushAsyncWork();

    const callbacks = sseState.callbacks;
    expect(callbacks).toBeDefined();

    const key = streamIdentity({
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
    });
    const metrics = {
      forwarder_endpoint_id: "fwd-1",
      stream_id: "stream-10.0.0.1:10000",
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

    store.streamEpochOptions = {
      [streamIdentity(streamA)]: [
        { stream_epoch: 5, name: null, first_seen_at: null, selectable: true },
      ],
      [streamIdentity(streamB)]: [
        { stream_epoch: 9, name: null, first_seen_at: null, selectable: true },
      ],
    };
    store.earliestEpochInputs = {
      [streamIdentity(streamA)]: "5",
      [streamIdentity(streamB)]: "9",
    };

    expect(selectedEarliestEpochValue(streamA)).toBe("5");
    expect(selectedEarliestEpochValue(streamB)).toBe("9");
  });

  it("formats epoch dropdown labels with created time or first-read time", async () => {
    const { formatEarliestEpochOption } = await import("./store.svelte");
    const created = 1_783_238_640_000;
    const firstSeen = "2026-07-05T09:51:11.314Z";

    expect(
      formatEarliestEpochOption({
        stream_epoch: 2,
        name: "Race Morning",
        first_seen_at: firstSeen,
        created_unix_ms: created,
        selectable: true,
      }),
    ).toBe(`#2 — Race Morning — ${new Date(created).toLocaleString()}`);
    expect(
      formatEarliestEpochOption({
        stream_epoch: 1,
        name: "  ",
        first_seen_at: firstSeen,
        selectable: true,
      }),
    ).toBe(`#1 — unnamed — first read ${new Date(firstSeen).toLocaleString()}`);
  });

  it("uses backend-merged epochs as-is (names and selectability come from the receiver)", async () => {
    const { store, streamIdentity, prefetchEarliestEpochOptions } =
      await import("./store.svelte");

    store.streamEpochOptions = {};
    store.earliestEpochLoading = {};
    apiMocks.getStreamEpochs.mockResolvedValueOnce({
      epochs: [
        {
          stream_epoch: 2,
          name: "Race Morning",
          first_seen_at: null,
          created_unix_ms: 1_783_238_640_000,
          selectable: true,
        },
        {
          stream_epoch: 1,
          name: null,
          first_seen_at: "2026-07-05T09:40:00.000Z",
          created_unix_ms: null,
          selectable: false,
        },
      ],
    });

    const stream = {
      forwarder_endpoint_id: "endpoint-1",
      stream_id: "11111111-1111-1111-1111-111111111111",
      subscribed: true,
      local_port: null,
      stream_epoch: 2,
      current_epoch_name: "Race Morning",
      current_epoch_created_unix_ms: 1_783_238_640_000,
    };

    await prefetchEarliestEpochOptions([stream]);

    expect(store.streamEpochOptions[streamIdentity(stream)]).toEqual([
      {
        stream_epoch: 2,
        name: "Race Morning",
        first_seen_at: null,
        created_unix_ms: 1_783_238_640_000,
        selectable: true,
      },
      {
        stream_epoch: 1,
        name: null,
        first_seen_at: "2026-07-05T09:40:00.000Z",
        created_unix_ms: null,
        selectable: false,
      },
    ]);
  });

  it("falls back to the advertised stream epoch when no events are stored yet", async () => {
    const { store, streamIdentity, prefetchEarliestEpochOptions } =
      await import("./store.svelte");

    store.streamEpochOptions = {};
    store.earliestEpochLoading = {};
    apiMocks.getStreamEpochs.mockResolvedValueOnce({ epochs: [] });

    const stream = {
      forwarder_endpoint_id: "endpoint-1",
      stream_id: "11111111-1111-1111-1111-111111111111",
      subscribed: true,
      local_port: null,
      stream_epoch: 12,
      current_epoch_name: "Race Morning",
      current_epoch_created_unix_ms: 1_783_238_640_000,
    };

    await prefetchEarliestEpochOptions([stream]);

    expect(apiMocks.getStreamEpochs).toHaveBeenCalledWith({
      forwarder_endpoint_id: "endpoint-1",
      stream_id: "11111111-1111-1111-1111-111111111111",
    });
    expect(store.streamEpochOptions[streamIdentity(stream)]).toEqual([
      {
        stream_epoch: 12,
        name: "Race Morning",
        first_seen_at: null,
        created_unix_ms: 1_783_238_640_000,
        selectable: true,
      },
    ]);
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
    expect(payload.streams).toEqual([
      { forwarder_id: "fwd-1", reader_ip: "10.0.0.1:10000" },
    ]);
  });
});
