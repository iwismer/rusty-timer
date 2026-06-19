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
            readers: [],
            ups: null,
            restart_needed: null,
          },
          {
            endpoint_id: "endpoint-2",
            display_name: null,
            state: "disconnected",
            pending: false,
            subscribed_count: 0,
            available_count: 1,
            readers: [],
            ups: null,
            restart_needed: false,
          },
          {
            endpoint_id: "endpoint-3",
            display_name: "Pending Forwarder",
            state: "disconnected",
            pending: true,
            subscribed_count: 0,
            available_count: 0,
            readers: [],
            ups: null,
            restart_needed: null,
          },
        ],
      },
    },
    connectForwarder: vi.fn(async () => {}),
    disconnectForwarder: vi.fn(async () => {}),
    reconnectForwarder: vi.fn(async () => {}),
    open: vi.fn(async () => {}),
  };
});

vi.mock("$lib/store.svelte", () => ({
  store: mockState.store,
}));

vi.mock("$lib/api", () => ({
  connectForwarder: mockState.connectForwarder,
  disconnectForwarder: mockState.disconnectForwarder,
  reconnectForwarder: mockState.reconnectForwarder,
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
          readers: [],
          ups: null,
          restart_needed: null,
        },
        {
          endpoint_id: "endpoint-2",
          display_name: null,
          state: "disconnected",
          pending: false,
          subscribed_count: 0,
          available_count: 1,
          readers: [],
          ups: null,
          restart_needed: false,
        },
        {
          endpoint_id: "endpoint-3",
          display_name: "Pending Forwarder",
          state: "disconnected",
          pending: true,
          subscribed_count: 0,
          available_count: 0,
          readers: [],
          ups: null,
          restart_needed: null,
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
        readers: [],
        ups: null,
        restart_needed: null,
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
        readers: [],
        ups: null,
        restart_needed: null,
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
