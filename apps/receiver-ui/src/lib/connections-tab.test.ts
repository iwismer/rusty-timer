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
      } as import("./api").ConnectionsResponse,
      streams: null as import("./api").StreamsResponse | null,
    },
    connectForwarder: vi.fn(async () => {}),
    disconnectForwarder: vi.fn(async () => {}),
    reconnectForwarder: vi.fn(async () => {}),
    loadConnections: vi.fn(async () => {}),
    refreshStreamsAndEpochOptions: vi.fn(async () => {}),
    openHelp: vi.fn(),
    readerControl: vi.fn(async () => ({
      success: true,
      message: "",
      reader_info: null,
    })),
    readerSetEpochName: vi.fn(async () => ({
      success: true,
      message: "",
      reader_info: null,
    })),
    readerAdvanceEpoch: vi.fn(async () => ({
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
  refreshStreamsAndEpochOptions: mockState.refreshStreamsAndEpochOptions,
  openHelp: mockState.openHelp,
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
  readerSetEpochName: mockState.readerSetEpochName,
  readerAdvanceEpoch: mockState.readerAdvanceEpoch,
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
    mockState.store.streams = null;
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
    } as import("./api").ConnectionsResponse;
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

  it("renders UPS live status without duplicate forwarder reader summary pills", () => {
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
      screen.queryByTestId("forwarder-reader-endpoint-live-10.0.0.1:10000"),
    ).not.toBeInTheDocument();
    expect(screen.getByTestId("forwarder-ups-endpoint-live")).toHaveTextContent(
      "64%",
    );
  });

  it("places forwarder actions above reader controls", async () => {
    mockState.store.connections.forwarders = [
      {
        endpoint_id: "endpoint-live",
        display_name: "Live Forwarder",
        state: "subscribed",
        pending: false,
        subscribed_count: 1,
        available_count: 1,
        readers: [
          {
            stream_id: "10.0.0.1:10000",
            connected: true,
            state: "online",
            last_read_unix_ms: null,
            hardware_reader_id: "reader-42",
            firmware_version: "1.2.3",
            model: "IPICO",
          },
        ],
        ups: null,
        restart_needed: null,
        remote_config_available: true,
        reader_control_available: true,
      },
    ] as import("./api").ForwarderConnectionStatus[];

    render(ConnectionsTab);

    const row = screen.getByTestId("forwarder-row-endpoint-live");
    expect(
      screen.queryByTestId("forwarder-reader-endpoint-live-10.0.0.1:10000"),
    ).not.toBeInTheDocument();
    await fireEvent.click(screen.getByLabelText("Show details"));
    const configure = screen.getByTestId("forwarder-configure-endpoint-live");
    const banner = within(row).getByText("Banner:");

    expect(
      configure.compareDocumentPosition(banner) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("labels reader controls by IP and shows hardware id plus local proxy status", () => {
    mockState.store.connections.forwarders = [
      {
        endpoint_id: "endpoint-live",
        display_name: "Live Forwarder",
        state: "subscribed",
        pending: false,
        subscribed_count: 1,
        available_count: 2,
        readers: [
          {
            stream_id: "10.0.0.1:10000",
            connected: true,
            state: "connected",
            last_read_unix_ms: null,
            hardware_reader_id: "0",
            firmware_version: "1.2.3",
            model: "IPICO",
            local_port: 9100,
          },
          {
            stream_id: "10.0.0.1:10001",
            connected: false,
            state: "disconnected",
            last_read_unix_ms: null,
            hardware_reader_id: "0",
            firmware_version: null,
            model: null,
            local_port: null,
          },
        ],
        ups: null,
        restart_needed: null,
        remote_config_available: false,
        reader_control_available: true,
      },
    ] as import("./api").ForwarderConnectionStatus[];

    render(ConnectionsTab);

    expect(
      screen.queryByTestId("forwarder-reader-endpoint-live-10.0.0.1:10000"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByTestId("forwarder-reader-endpoint-live-10.0.0.1:10001"),
    ).not.toBeInTheDocument();

    // Reader identity now lives in the wrapping card header rather than the
    // control panel's own header.
    const row = screen.getByTestId("forwarder-row-endpoint-live");
    expect(row).toHaveTextContent("10.0.0.1:10000");
    expect(row).toHaveTextContent("connected to forwarder");
    expect(row).toHaveTextContent("10.0.0.1:10001");
    expect(row).toHaveTextContent("disconnected from forwarder");

    const panels = screen.getAllByTestId("reader-control-panel");
    expect(panels).toHaveLength(2);
    expect(panels[0]).toHaveTextContent("Local proxy: 127.0.0.1:9100");
    expect(panels[1]).toHaveTextContent("Local proxy: not subscribed");
  });

  it("shows reads counters, last seen, epoch name, and collapsible details", async () => {
    mockState.store.connections.forwarders = [
      {
        endpoint_id: "endpoint-live",
        display_name: "Live Forwarder",
        state: "subscribed",
        pending: false,
        subscribed_count: 1,
        available_count: 1,
        readers: [
          {
            stream_id: "10.0.0.1:10000",
            connected: true,
            state: "online",
            last_read_unix_ms: null,
            reads_session: 12,
            reads_total: 3456,
            last_seen_secs: 5,
            current_epoch: 9,
            current_epoch_created_unix_ms: Date.UTC(2026, 6, 5, 8, 24),
            current_epoch_name: "Race Day",
            hardware_reader_id: "reader-42",
            firmware_version: "1.2.3",
            model: "69",
            local_port: 9100,
          },
        ],
        ups: null,
        restart_needed: null,
        remote_config_available: false,
        reader_control_available: true,
      },
    ] as import("./api").ForwarderConnectionStatus[];
    mockState.store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "endpoint-live",
          stream_id: "10.0.0.1:10000",
          subscribed: true,
          local_port: 9100,
          stream_epoch: 7,
          current_epoch_name: "Race Day",
        },
      ],
      degraded: false,
      upstream_error: null,
    };

    render(ConnectionsTab);

    const panel = screen.getByTestId("reader-control-panel");
    expect(panel).toHaveTextContent("Reads (session): 12");
    expect(panel).toHaveTextContent("Reads (total): 3,456");
    expect(panel).toHaveTextContent("Last seen: 5s ago");
    expect(panel).toHaveTextContent("Current epoch: #9");
    expect(panel).toHaveTextContent("Name: Race Day");
    expect(panel).not.toHaveTextContent("Created: —");

    // Details start collapsed and can be expanded.
    expect(screen.queryByText("Banner:")).not.toBeInTheDocument();
    await fireEvent.click(screen.getByLabelText("Show details"));
    expect(screen.getByText("Banner:")).toBeInTheDocument();
    // Hardware code renders both hex and decimal forms.
    expect(panel).toHaveTextContent("Hardware: 0x45 (69)");
    await fireEvent.click(screen.getByLabelText("Hide details"));
    expect(screen.queryByText("Banner:")).not.toBeInTheDocument();
  });

  it("shows unnamed for receiver readers without an epoch name", () => {
    mockState.store.connections.forwarders = [
      {
        endpoint_id: "endpoint-live",
        display_name: "Live Forwarder",
        state: "subscribed",
        pending: false,
        subscribed_count: 1,
        available_count: 1,
        readers: [
          {
            stream_id: "10.0.0.1:10000",
            connected: true,
            state: "online",
            last_read_unix_ms: null,
            current_epoch: 8,
            current_epoch_name: null,
            hardware_reader_id: null,
            firmware_version: null,
            model: null,
          },
        ],
        ups: null,
        restart_needed: null,
        remote_config_available: false,
        reader_control_available: true,
      },
    ] as import("./api").ForwarderConnectionStatus[];
    mockState.store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "endpoint-live",
          stream_id: "10.0.0.1:10000",
          subscribed: true,
          local_port: 9100,
          stream_epoch: 8,
          current_epoch_name: null,
        },
      ],
      degraded: false,
      upstream_error: null,
    };

    render(ConnectionsTab);

    const panel = screen.getByTestId("reader-control-panel");
    expect(panel).toHaveTextContent("Current epoch: #8");
    expect(panel).toHaveTextContent("Name: unnamed");
  });

  it("wires epoch name save and advance epoch to the reader commands", async () => {
    mockState.store.connections.forwarders = [
      {
        endpoint_id: "endpoint-live",
        display_name: "Live Forwarder",
        state: "subscribed",
        pending: false,
        subscribed_count: 1,
        available_count: 1,
        readers: [
          {
            stream_id: "10.0.0.1:10000",
            connected: true,
            state: "online",
            last_read_unix_ms: null,
            hardware_reader_id: "reader-42",
            firmware_version: "1.2.3",
            model: "IPICO",
          },
        ],
        ups: null,
        restart_needed: null,
        remote_config_available: false,
        reader_control_available: true,
      },
    ] as import("./api").ForwarderConnectionStatus[];

    render(ConnectionsTab);

    const input = screen.getByPlaceholderText("Set epoch name");
    await fireEvent.input(input, { target: { value: "Lap 2" } });
    await fireEvent.click(screen.getByText("Save"));

    expect(mockState.readerSetEpochName).toHaveBeenCalledWith(
      "endpoint-live",
      "10.0.0.1:10000",
      "Lap 2",
    );
    expect(mockState.refreshStreamsAndEpochOptions).toHaveBeenCalledWith([
      {
        forwarder_endpoint_id: "endpoint-live",
        stream_id: "10.0.0.1:10000",
      },
    ]);
    expect(await screen.findByText("Epoch name saved")).toBeInTheDocument();

    await fireEvent.click(screen.getByText("Advance Epoch"));

    expect(mockState.readerAdvanceEpoch).toHaveBeenCalledWith(
      "endpoint-live",
      "10.0.0.1:10000",
    );
    expect(mockState.readerSetEpochName).toHaveBeenLastCalledWith(
      "endpoint-live",
      "10.0.0.1:10000",
      "Lap 2",
    );
    expect(mockState.refreshStreamsAndEpochOptions).toHaveBeenLastCalledWith([
      {
        forwarder_endpoint_id: "endpoint-live",
        stream_id: "10.0.0.1:10000",
      },
    ]);
    expect(
      await screen.findByText("Advanced to next epoch and saved name"),
    ).toBeInTheDocument();
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
        .map((b) => b.textContent)
        .filter((text) => text !== "?"),
    ).toEqual(["Reconnect", "Disconnect"]);
  });

  it("shows reconnect but disables other controls for disconnected readers", async () => {
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
            hardware_reader_id: null,
            firmware_version: null,
            model: null,
          },
        ],
        ups: null as import("./api").UpsStatusPayload | null,
        restart_needed: null,
        remote_config_available: false,
        reader_control_available: true,
      },
    ] as import("./api").ForwarderConnectionStatus[];

    render(ConnectionsTab);

    await fireEvent.click(screen.getByLabelText("Show details"));
    expect(screen.getByText("No reader data available")).toBeInTheDocument();
    expect(screen.getByText("Sync Clock")).toBeDisabled();
    expect(screen.getAllByText("Reconnect")[0]).toBeEnabled();
  });

  it("surfaces reader command failures from successful API responses", async () => {
    mockState.readerControl.mockResolvedValueOnce({
      success: false,
      message: "reader not connected",
      reader_info: null,
    });
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
            connected: true,
            state: "online",
            last_read_unix_ms: null,
            hardware_reader_id: "reader-42",
            firmware_version: "1.2.3",
            model: "IPICO",
          },
        ],
        ups: null as import("./api").UpsStatusPayload | null,
        restart_needed: null,
        remote_config_available: false,
        reader_control_available: true,
      },
    ] as import("./api").ForwarderConnectionStatus[];

    render(ConnectionsTab);

    await fireEvent.click(screen.getByLabelText("Show details"));
    await fireEvent.click(screen.getByText("Sync Clock"));

    expect(
      await screen.findByText("Sync Clock failed: reader not connected"),
    ).toBeInTheDocument();
    expect(screen.queryByText("Clock synced")).not.toBeInTheDocument();
  });

  it("renders unknown connection states with a safe fallback", () => {
    mockState.store.connections.forwarders = [
      {
        endpoint_id: "endpoint-unknown",
        display_name: "Unknown Forwarder",
        state: "unexpected" as import("./api").ForwarderConnState,
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
