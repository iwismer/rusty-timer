import { describe, it, expect, vi, beforeEach } from "vitest";

// Mock @tauri-apps/api/core
const mockInvoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: mockInvoke,
}));

// Mock @tauri-apps/api/event for sse.ts tests
const mockListen = vi.fn();
vi.mock("@tauri-apps/api/event", () => ({
  listen: mockListen,
}));

beforeEach(() => {
  mockInvoke.mockReset();
  mockListen.mockReset();
});

describe("api client", () => {
  it("getProfile calls correct command", async () => {
    const { getProfile } = await import("./api");
    mockInvoke.mockResolvedValue({
      server_url: "https://thin.test",
      token: "tok",
      receiver_id: "recv-test",
    });
    const p = await getProfile();
    expect(mockInvoke).toHaveBeenCalledWith("get_profile");
    expect(p.server_url).toBe("https://thin.test");
    expect(p.receiver_id).toBe("recv-test");
  });

  it("putProfile sends body argument", async () => {
    const { putProfile } = await import("./api");
    mockInvoke.mockResolvedValue(undefined);
    await putProfile({
      server_url: "https://thin.test",
      token: "t",
      receiver_id: "recv-test",
    });
    expect(mockInvoke).toHaveBeenCalledWith("put_profile", {
      body: {
        server_url: "https://thin.test",
        token: "t",
        receiver_id: "recv-test",
      },
    });
  });

  it("getStreams returns streams response", async () => {
    const { getStreams } = await import("./api");
    mockInvoke.mockResolvedValue({
      streams: [],
      degraded: false,
      upstream_error: null,
    });
    const r = await getStreams();
    expect(mockInvoke).toHaveBeenCalledWith("get_streams");
    expect(r.degraded).toBe(false);
    expect(r.streams).toEqual([]);
  });

  it("getStatus returns status", async () => {
    const { getStatus } = await import("./api");
    mockInvoke.mockResolvedValue({
      connection_state: "disconnected",
      local_ok: true,
      streams_count: 0,
      receiver_id: "recv-status",
      server: {
        configured: true,
        endpoint_id: "node-1",
        reachable: true,
        approval_state: "pending",
        waiting_for_approval: true,
        message: "Waiting for server admin approval",
      },
    });
    const s = await getStatus();
    expect(mockInvoke).toHaveBeenCalledWith("get_status");
    expect(s.connection_state).toBe("disconnected");
    expect(s.receiver_id).toBe("recv-status");
    expect(s.server.waiting_for_approval).toBe(true);
  });

  it("reconnectServer calls reconnect command", async () => {
    const { reconnectServer } = await import("./api");
    mockInvoke.mockResolvedValue(undefined);

    await reconnectServer();

    expect(mockInvoke).toHaveBeenCalledWith("reconnect_server");
  });

  it("getConnections calls connections command", async () => {
    const { getConnections } = await import("./api");
    mockInvoke.mockResolvedValue({
      server: {
        configured: true,
        endpoint_id: "server-node-1",
        reachable: true,
        approval_state: "active",
        waiting_for_approval: false,
        message: null,
      },
      forwarders: [],
    });

    const response = await getConnections();

    expect(mockInvoke).toHaveBeenCalledWith("get_connections");
    expect(response.forwarders).toEqual([]);
  });

  it("sends camelCase endpoint id for forwarder connection commands", async () => {
    const { connectForwarder, disconnectForwarder, reconnectForwarder } =
      await import("./api");
    mockInvoke.mockResolvedValue(undefined);

    await connectForwarder("endpoint-1");
    await disconnectForwarder("endpoint-1");
    await reconnectForwarder("endpoint-1");

    expect(mockInvoke).toHaveBeenNthCalledWith(1, "connect_forwarder", {
      endpointId: "endpoint-1",
    });
    expect(mockInvoke).toHaveBeenNthCalledWith(2, "disconnect_forwarder", {
      endpointId: "endpoint-1",
    });
    expect(mockInvoke).toHaveBeenNthCalledWith(3, "reconnect_forwarder", {
      endpointId: "endpoint-1",
    });
  });

  it("sends camelCase args for remote forwarder config commands", async () => {
    const { getForwarderConfig, setForwarderConfig, restartForwarder } =
      await import("./api");
    mockInvoke
      .mockResolvedValueOnce({ config_json: "{}", restart_needed: false })
      .mockResolvedValueOnce({ ok: true, restart_needed: true, error: null })
      .mockResolvedValueOnce({ accepted: true, error: null });

    await getForwarderConfig("endpoint-1");
    await setForwarderConfig("endpoint-1", '{"display_name":"A"}');
    await restartForwarder("endpoint-1");

    expect(mockInvoke).toHaveBeenNthCalledWith(1, "get_forwarder_config", {
      endpointId: "endpoint-1",
    });
    expect(mockInvoke).toHaveBeenNthCalledWith(2, "set_forwarder_config", {
      endpointId: "endpoint-1",
      configJson: '{"display_name":"A"}',
    });
    expect(mockInvoke).toHaveBeenNthCalledWith(3, "restart_forwarder", {
      endpointId: "endpoint-1",
    });
  });

  it("putSubscriptions sends body with subscriptions", async () => {
    const { putSubscriptions } = await import("./api");
    mockInvoke.mockResolvedValue(undefined);
    await putSubscriptions([
      {
        forwarder_endpoint_id: "endpoint-1",
        stream_id: "11111111-1111-1111-1111-111111111111",
        local_port_override: null,
      },
    ]);
    expect(mockInvoke).toHaveBeenCalledWith("put_subscriptions", {
      body: {
        subscriptions: [
          {
            forwarder_endpoint_id: "endpoint-1",
            stream_id: "11111111-1111-1111-1111-111111111111",
            local_port_override: null,
          },
        ],
      },
    });
  });

  it("rejects when invoke rejects", async () => {
    const { getProfile } = await import("./api");
    mockInvoke.mockRejectedValue("internal error");
    await expect(getProfile()).rejects.toBe("internal error");
  });

  it("getMode calls mode command", async () => {
    const { getMode } = await import("./api");
    mockInvoke.mockResolvedValue({
      mode: "live",
      streams: [],
      earliest_epochs: [],
    });
    const result = await getMode();
    expect(mockInvoke).toHaveBeenCalledWith("get_mode");
    expect(result.mode).toBe("live");
  });

  it("putMode sends mode argument", async () => {
    const { putMode } = await import("./api");
    mockInvoke.mockResolvedValue(undefined);
    const payload: Parameters<typeof putMode>[0] = {
      mode: "race",
      race_id: "race-1",
    };
    await putMode(payload);

    expect(mockInvoke).toHaveBeenCalledWith("put_mode", {
      mode: payload,
    });
  });

  it("putEarliestEpoch sends body argument", async () => {
    const { putEarliestEpoch } = await import("./api");
    mockInvoke.mockResolvedValue(undefined);
    const payload = {
      forwarder_endpoint_id: "endpoint-1",
      stream_id: "11111111-1111-1111-1111-111111111111",
      earliest_epoch: 7,
    };
    await putEarliestEpoch(payload);
    expect(mockInvoke).toHaveBeenCalledWith("put_earliest_epoch", {
      body: payload,
    });
  });

  it("updateSubscriptionEventType sends camelCase top-level Tauri args", async () => {
    const { updateSubscriptionEventType } = await import("./api");
    mockInvoke.mockResolvedValue(undefined);

    await updateSubscriptionEventType(
      {
        forwarder_endpoint_id: "endpoint-1",
        stream_id: "11111111-1111-1111-1111-111111111111",
      },
      "start",
    );

    expect(mockInvoke).toHaveBeenCalledWith("update_subscription_event_type", {
      forwarderEndpointId: "endpoint-1",
      streamId: "11111111-1111-1111-1111-111111111111",
      body: { event_type: "start" },
    });
  });

  it("getReplayTargetEpochs calls command with stream params", async () => {
    const { getReplayTargetEpochs } = await import("./api");
    mockInvoke.mockResolvedValue({
      epochs: [
        {
          stream_epoch: 7,
          name: "Heat 2",
          first_seen_at: "2026-02-01T10:00:00Z",
          race_names: ["Saturday 5K"],
        },
      ],
    });

    const result = await getReplayTargetEpochs({
      stream_id: "10.0.0.1:10000",
    });

    expect(mockInvoke).toHaveBeenCalledWith("get_replay_target_epochs", {
      streamId: "10.0.0.1:10000",
    });
    expect(result.epochs).toEqual([
      {
        stream_epoch: 7,
        name: "Heat 2",
        first_seen_at: "2026-02-01T10:00:00Z",
        race_names: ["Saturday 5K"],
      },
    ]);
  });

  it("resetStreamCursor calls admin_reset_cursor with body", async () => {
    const { resetStreamCursor } = await import("./api");
    mockInvoke.mockResolvedValue(undefined);

    await resetStreamCursor({
      stream_id: "11111111-1111-1111-1111-111111111111",
    });

    expect(mockInvoke).toHaveBeenCalledWith("admin_reset_cursor", {
      body: {
        stream_id: "11111111-1111-1111-1111-111111111111",
      },
    });
  });
});

describe("sse client", () => {
  beforeEach(() => {
    vi.resetModules();
    mockListen.mockReset();
  });

  it("registers Tauri event listeners and signals connected", async () => {
    mockListen.mockResolvedValue(() => {});

    const { initSSE, destroySSE } = await import("./sse");
    const callbacks = {
      onStatusChanged: vi.fn(),
      onStreamsSnapshot: vi.fn(),
      onLogEntry: vi.fn(),
      onResync: vi.fn(),
      onConnectionsChanged: vi.fn(),
      onConnectionChange: vi.fn(),
      onStreamCountsUpdated: vi.fn(),
      onForwarderMetricsUpdated: vi.fn(),
      onModeChanged: vi.fn(),
      onLastRead: vi.fn(),
      onStreamMetricsUpdated: vi.fn(),
    };

    await initSSE(callbacks);

    // Should immediately signal connected
    expect(callbacks.onConnectionChange).toHaveBeenCalledWith(true);

    // Should register listeners for all event types
    const registeredEvents = mockListen.mock.calls.map(
      (call: any[]) => call[0],
    );
    expect(registeredEvents).toContain("status_changed");
    expect(registeredEvents).toContain("streams_snapshot");
    expect(registeredEvents).toContain("log_entry");
    expect(registeredEvents).toContain("resync");
    expect(registeredEvents).toContain("connections_changed");
    expect(registeredEvents).toContain("stream_counts_updated");
    expect(registeredEvents).toContain("forwarder_metrics_updated");
    expect(registeredEvents).toContain("mode_changed");
    expect(registeredEvents).toContain("last_read");

    destroySSE();
  });

  it("forwards stream_counts_updated event payload", async () => {
    mockListen.mockImplementation(
      async (eventName: string, callback: (event: any) => void) => {
        if (eventName === "stream_counts_updated") {
          // Simulate an event firing
          setTimeout(() => {
            callback({
              payload: {
                updates: [
                  {
                    forwarder_id: "f1",
                    reader_ip: "10.0.0.1",
                    reads_total: 15,
                    reads_epoch: 3,
                  },
                ],
              },
            });
          }, 0);
        }
        return () => {};
      },
    );

    const { initSSE, destroySSE } = await import("./sse");
    const callbacks = {
      onStatusChanged: vi.fn(),
      onStreamsSnapshot: vi.fn(),
      onLogEntry: vi.fn(),
      onResync: vi.fn(),
      onConnectionsChanged: vi.fn(),
      onConnectionChange: vi.fn(),
      onStreamCountsUpdated: vi.fn(),
      onForwarderMetricsUpdated: vi.fn(),
      onModeChanged: vi.fn(),
      onLastRead: vi.fn(),
      onStreamMetricsUpdated: vi.fn(),
    };

    await initSSE(callbacks);

    // Wait for the setTimeout callback to fire
    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(callbacks.onStreamCountsUpdated).toHaveBeenCalledWith([
      {
        forwarder_id: "f1",
        reader_ip: "10.0.0.1",
        reads_total: 15,
        reads_epoch: 3,
      },
    ]);

    destroySSE();
  });

  it("forwards forwarder_metrics_updated event payload", async () => {
    mockListen.mockImplementation(
      async (eventName: string, callback: (event: any) => void) => {
        if (eventName === "forwarder_metrics_updated") {
          setTimeout(() => {
            callback({
              payload: {
                forwarder_id: "f1",
                unique_chips: 4,
                total_reads: 15,
                last_read_at: "2026-03-21T12:34:56.000Z",
              },
            });
          }, 0);
        }
        return () => {};
      },
    );

    const { initSSE, destroySSE } = await import("./sse");
    const callbacks = {
      onStatusChanged: vi.fn(),
      onStreamsSnapshot: vi.fn(),
      onLogEntry: vi.fn(),
      onResync: vi.fn(),
      onConnectionsChanged: vi.fn(),
      onConnectionChange: vi.fn(),
      onStreamCountsUpdated: vi.fn(),
      onForwarderMetricsUpdated: vi.fn(),
      onStreamMetricsUpdated: vi.fn(),
      onModeChanged: vi.fn(),
      onLastRead: vi.fn(),
    };

    await initSSE(callbacks);
    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(callbacks.onForwarderMetricsUpdated).toHaveBeenCalledWith({
      forwarder_id: "f1",
      unique_chips: 4,
      total_reads: 15,
      last_read_at: "2026-03-21T12:34:56.000Z",
    });

    destroySSE();
  });

  it("forwards mode_changed event payload", async () => {
    mockListen.mockImplementation(
      async (eventName: string, callback: (event: any) => void) => {
        if (eventName === "mode_changed") {
          setTimeout(() => {
            callback({
              payload: {
                mode: { mode: "race", race_id: "race-1" },
              },
            });
          }, 0);
        }
        return () => {};
      },
    );

    const { initSSE, destroySSE } = await import("./sse");
    const callbacks = {
      onStatusChanged: vi.fn(),
      onStreamsSnapshot: vi.fn(),
      onLogEntry: vi.fn(),
      onResync: vi.fn(),
      onConnectionsChanged: vi.fn(),
      onConnectionChange: vi.fn(),
      onStreamCountsUpdated: vi.fn(),
      onForwarderMetricsUpdated: vi.fn(),
      onModeChanged: vi.fn(),
      onLastRead: vi.fn(),
      onStreamMetricsUpdated: vi.fn(),
    };

    await initSSE(callbacks);

    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(callbacks.onModeChanged).toHaveBeenCalledWith({
      mode: "race",
      race_id: "race-1",
    });
    destroySSE();
  });
});
