import { fireEvent, render, screen, within } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import ConnectionsTab from "./components/ConnectionsTab.svelte";

const mockState = vi.hoisted(() => {
  const server = () => ({
    configured: true,
    endpoint_id: "server-node-1" as string | null,
    reachable: true as boolean | null,
    approval_state: "active" as string | null,
    waiting_for_approval: false,
    message: null as string | null,
  });
  const readers = () => [] as import("./api").ReaderLiveStatus[];
  const ups = () => null as import("./api").UpsStatusPayload | null;

  return {
    store: {
      savedServerUrl: "https://server.example.com/",
      connections: {
        server: server(),
        forwarders: [
          {
            endpoint_id: "endpoint-1",
            display_name: "Timing Forwarder",
            state: "subscribed",
            pending: false,
            subscribed_count: 2,
            available_count: 3,
            readers: readers(),
            ups: ups(),
            restart_needed: null,
            remote_config_available: true,
          },
          {
            endpoint_id: "endpoint-2",
            display_name: null,
            state: "disconnected",
            pending: false,
            subscribed_count: 0,
            available_count: 1,
            readers: readers(),
            ups: ups(),
            restart_needed: false,
            remote_config_available: false,
          },
          {
            endpoint_id: "endpoint-3",
            display_name: "Pending Forwarder",
            state: "disconnected",
            pending: true,
            subscribed_count: 0,
            available_count: 0,
            readers: readers(),
            ups: ups(),
            restart_needed: null,
            remote_config_available: true,
          },
        ],
      },
    },
    connectForwarder: vi.fn(async () => {}),
    disconnectForwarder: vi.fn(async () => {}),
    reconnectForwarder: vi.fn(async () => {}),
    loadConnections: vi.fn(async () => {}),
    readerControl: vi.fn(async () => ({
      success: true,
      message: "",
      reader_info: null,
    })),
    open: vi.fn(async () => {}),
  };
});

vi.mock("$lib/store.svelte", () => ({
  store: mockState.store,
  loadConnections: mockState.loadConnections,
}));

vi.mock("$lib/api", () => ({
  connectForwarder: mockState.connectForwarder,
  disconnectForwarder: mockState.disconnectForwarder,
  reconnectForwarder: mockState.reconnectForwarder,
  readerClearRecords: mockState.readerControl,
  readerGetInfo: mockState.readerControl,
  readerReconnect: mockState.readerControl,
  readerRefresh: mockState.readerControl,
  readerSetReadMode: mockState.readerControl,
  readerSetRecording: mockState.readerControl,
  readerSetTto: mockState.readerControl,
  readerStartDownload: mockState.readerControl,
  readerStopDownload: mockState.readerControl,
  readerSyncClock: mockState.readerControl,
  getForwarderConfig: vi.fn(),
  setForwarderConfig: vi.fn(),
  restartForwarder: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: mockState.open,
}));

describe("ConnectionsTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockState.store.savedServerUrl = "https://server.example.com/";
    mockState.store.connections = {
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
          endpoint_id: "endpoint-1",
          display_name: "Timing Forwarder",
          state: "subscribed",
          pending: false,
          subscribed_count: 2,
          available_count: 3,
          readers: [] as import("./api").ReaderLiveStatus[],
          ups: null as import("./api").UpsStatusPayload | null,
          restart_needed: null,
          remote_config_available: true,
        },
        {
          endpoint_id: "endpoint-2",
          display_name: null,
          state: "disconnected",
          pending: false,
          subscribed_count: 0,
          available_count: 1,
          readers: [] as import("./api").ReaderLiveStatus[],
          ups: null as import("./api").UpsStatusPayload | null,
          restart_needed: false,
          remote_config_available: false,
        },
        {
          endpoint_id: "endpoint-3",
          display_name: "Pending Forwarder",
          state: "disconnected",
          pending: true,
          subscribed_count: 0,
          available_count: 0,
          readers: [] as import("./api").ReaderLiveStatus[],
          ups: null as import("./api").UpsStatusPayload | null,
          restart_needed: null,
          remote_config_available: true,
        },
      ],
    };
  });

  it("renders the server card and forwarder state rows", () => {
    render(ConnectionsTab);

    expect(screen.getByTestId("connections-server-card")).toBeInTheDocument();
    expect(screen.getByTestId("server-approval-state")).toHaveTextContent(
      "Server approved",
    );
    expect(screen.getByTestId("forwarder-row-endpoint-1")).toHaveTextContent(
      "Timing Forwarder",
    );
    expect(screen.getByTestId("forwarder-state-endpoint-1")).toHaveTextContent(
      "Subscribed",
    );
    expect(screen.getByTestId("forwarder-row-endpoint-1")).toHaveTextContent(
      "2 subscribed / 3 available",
    );
    expect(screen.getByTestId("forwarder-state-endpoint-2")).toHaveTextContent(
      "Disconnected",
    );
    expect(screen.getByTestId("forwarder-state-endpoint-3")).toHaveTextContent(
      "Connecting…",
    );
  });

  it("renders forwarder reader and UPS live status", () => {
    mockState.store.connections.forwarders = [
      {
        endpoint_id: "endpoint-live",
        display_name: "Live Forwarder",
        state: "connected",
        pending: false,
        subscribed_count: 1,
        available_count: 1,
        readers: [
          {
            stream_id: "10.0.0.1:10000",
            connected: false,
            state: "offline",
            last_read_unix_ms: null,
            hardware_reader_id: "reader-42",
            firmware_version: "1.2.3",
            model: "IPICO",
          },
        ],
        ups: {
          on_battery: true,
          battery_percent: 64,
          runtime_seconds: 900,
        },
        restart_needed: null,
        remote_config_available: false,
      },
    ];

    render(ConnectionsTab);

    expect(
      screen.getByTestId("forwarder-reader-endpoint-live-10.0.0.1:10000"),
    ).toHaveTextContent("reader-42");
    expect(
      screen.getByTestId("forwarder-reader-endpoint-live-10.0.0.1:10000"),
    ).toHaveTextContent("offline");
    expect(screen.getByTestId("forwarder-ups-endpoint-live")).toHaveTextContent(
      "64%",
    );
  });

  it("shows configure only for forwarders that support remote config", () => {
    render(ConnectionsTab);

    expect(
      screen.getByTestId("forwarder-configure-endpoint-1"),
    ).toBeInTheDocument();
    expect(
      screen.queryByTestId("forwarder-configure-endpoint-2"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByTestId("forwarder-configure-endpoint-3"),
    ).toBeInTheDocument();
  });

  it("calls the matching per-forwarder actions with the endpoint id", async () => {
    render(ConnectionsTab);

    await fireEvent.click(
      screen.getByTestId("forwarder-disconnect-endpoint-1"),
    );
    await fireEvent.click(screen.getByTestId("forwarder-connect-endpoint-2"));
    await fireEvent.click(screen.getByTestId("forwarder-reconnect-endpoint-3"));

    expect(mockState.disconnectForwarder).toHaveBeenCalledWith("endpoint-1");
    expect(mockState.connectForwarder).toHaveBeenCalledWith("endpoint-2");
    expect(mockState.reconnectForwarder).toHaveBeenCalledWith("endpoint-3");
  });

  it("shows reconnect before disconnect for unavailable forwarders", () => {
    mockState.store.connections.forwarders = [
      {
        endpoint_id: "endpoint-unavailable",
        display_name: "Unavailable Forwarder",
        state: "unavailable",
        pending: false,
        subscribed_count: 0,
        available_count: 0,
        readers: [] as import("./api").ReaderLiveStatus[],
        ups: null as import("./api").UpsStatusPayload | null,
        restart_needed: null,
        remote_config_available: false,
      },
    ];

    render(ConnectionsTab);

    const row = screen.getByTestId("forwarder-row-endpoint-unavailable");
    expect(
      within(row)
        .getAllByRole("button")
        .map((b) => b.textContent),
    ).toEqual(["Reconnect", "Disconnect"]);
  });

  it("renders unknown connection states with a safe fallback", () => {
    mockState.store.connections.forwarders = [
      {
        endpoint_id: "endpoint-unknown",
        display_name: "Unknown Forwarder",
        state: "unexpected",
        pending: false,
        subscribed_count: 0,
        available_count: 0,
        readers: [] as import("./api").ReaderLiveStatus[],
        ups: null as import("./api").UpsStatusPayload | null,
        restart_needed: null,
        remote_config_available: false,
      },
    ];

    render(ConnectionsTab);

    expect(
      screen.getByTestId("forwarder-state-endpoint-unknown"),
    ).toHaveTextContent("Unknown");
  });

  it("opens the server admin panel in the system browser", async () => {
    render(ConnectionsTab);

    await fireEvent.click(screen.getByTestId("open-admin-panel-btn"));

    expect(mockState.open).toHaveBeenCalledWith(
      "https://server.example.com/admin",
    );
  });
});
