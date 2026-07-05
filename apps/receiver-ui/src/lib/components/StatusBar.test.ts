import { render, screen } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import StatusBar from "./StatusBar.svelte";

const mockState = vi.hoisted(() => {
  const store = {
    streams: { streams: [] as import("$lib/api").StreamEntry[] },
    status: { receiver_id: "recv-test" },
    connections: null,
    appVersion: "0.8.0",
    updateState: null,
  };
  return {
    store,
    health: "warn" as "ok" | "warn" | "err",
    getOverallHealth: vi.fn(() => mockState.health),
    openHelp: vi.fn(),
    openUpdateModal: vi.fn(),
  };
});

vi.mock("$lib/store.svelte", () => ({
  store: mockState.store,
  getOverallHealth: mockState.getOverallHealth,
  openHelp: mockState.openHelp,
  openUpdateModal: mockState.openUpdateModal,
}));

function setHealth(health: "ok" | "warn" | "err") {
  mockState.health = health;
}

describe("StatusBar aggregate health dot", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockState.store.streams = {
      streams: [] as import("$lib/api").StreamEntry[],
    };
    mockState.store.status = { receiver_id: "recv-test" };
    mockState.store.connections = null;
    mockState.store.appVersion = "0.8.0";
    mockState.store.updateState = null;
  });

  it("shows green when the server is approved and all forwarders are connected or subscribed", () => {
    setHealth("ok");

    render(StatusBar);

    const dot = screen.getByTestId("overall-health-dot");
    expect(dot).toHaveAttribute("data-health", "ok");
    expect(dot).toHaveAttribute("aria-label", "All connected");
    expect(dot).toHaveClass("bg-status-ok");
  });

  it("shows red when the server is unreachable or all forwarders are unavailable", () => {
    setHealth("err");

    render(StatusBar);

    const dot = screen.getByTestId("overall-health-dot");
    expect(dot).toHaveAttribute("data-health", "err");
    expect(dot).toHaveAttribute("aria-label", "Disconnected");
    expect(dot).toHaveClass("bg-status-err");
  });

  it("shows orange when approval is pending or forwarders are degraded", () => {
    setHealth("warn");

    render(StatusBar);

    const dot = screen.getByTestId("overall-health-dot");
    expect(dot).toHaveAttribute("data-health", "warn");
    expect(dot).toHaveAttribute("aria-label", "Some connections degraded");
    expect(dot).toHaveClass("bg-status-warn");
  });

  it("does not render separate stream health dots beside the aggregate dot", () => {
    setHealth("ok");
    mockState.store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "endpoint-1",
          stream_id: "stream-1",
          subscribed: true,
          local_port: 10100,
          online: true,
          reader_connected: true,
          reads_total: 3,
        },
        {
          forwarder_endpoint_id: "endpoint-1",
          stream_id: "stream-2",
          subscribed: true,
          local_port: 10101,
          online: true,
          reader_connected: false,
          reads_total: 4,
        },
      ],
    };

    render(StatusBar);

    expect(screen.getByTestId("overall-health-dot")).toBeInTheDocument();
    expect(screen.queryByText("online")).not.toBeInTheDocument();
    expect(screen.queryByText("degraded")).not.toBeInTheDocument();
    expect(screen.getByText("7 reads")).toBeInTheDocument();
  });
});
