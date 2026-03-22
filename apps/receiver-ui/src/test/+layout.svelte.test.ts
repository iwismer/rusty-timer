import { render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import Layout from "../routes/+layout.svelte";
import LayoutChildrenHarness from "./LayoutChildrenHarness.svelte";
import { store } from "$lib/store.svelte";

const apiMocks = vi.hoisted(() => ({
  getStatus: vi.fn().mockResolvedValue({
    connection_state: "connected",
    local_ok: true,
    streams_count: 1,
    receiver_id: "recv-test",
  }),
  getStreams: vi.fn().mockResolvedValue({
    streams: [
      {
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.1:10000",
        subscribed: true,
        local_port: 12484,
        stream_epoch: 5,
        reads_total: 0,
        reads_epoch: 0,
      },
    ],
    degraded: false,
    upstream_error: null,
  }),
  getLogs: vi.fn().mockResolvedValue({ entries: [] }),
  getProfile: vi.fn().mockResolvedValue(null),
  getMode: vi.fn().mockResolvedValue({
    mode: "live",
    streams: [],
    earliest_epochs: [],
  }),
  getRaces: vi.fn().mockResolvedValue({ races: [] }),
  getReplayTargetEpochs: vi.fn().mockResolvedValue({
    epochs: [
      { stream_epoch: 5, name: "Main", first_seen_at: null, race_names: [] },
    ],
  }),
  getForwarders: vi.fn().mockResolvedValue({ forwarders: [] }),
  putMode: vi.fn().mockResolvedValue(undefined),
  putProfile: vi.fn().mockResolvedValue(undefined),
  connect: vi.fn().mockResolvedValue(undefined),
  disconnect: vi.fn().mockResolvedValue(undefined),
  putEarliestEpoch: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("$lib/api", () => apiMocks);

const sseMocks = vi.hoisted(() => ({
  initSSE: vi.fn(),
  destroySSE: vi.fn(),
}));

const pageState = vi.hoisted(() => ({
  pathname: "/",
}));

vi.mock("$lib/sse", () => ({
  initSSE: sseMocks.initSSE,
  destroySSE: sseMocks.destroySSE,
}));

vi.mock("$app/state", () => ({
  page: {
    get url() {
      return new URL(`http://localhost${pageState.pathname}`);
    },
  },
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

describe("receiver layout SSE updates", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    pageState.pathname = "/";
    store.activeTab = "streams";
    store.forwarders = null;
    store.forwardersError = null;
    store.selectedForwarderId = null;
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        disconnect() {}
        unobserve() {}
      },
    );
    vi.stubGlobal("localStorage", {
      getItem: vi.fn().mockReturnValue(null),
      setItem: vi.fn(),
      removeItem: vi.fn(),
    });
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {
        transformCallback: vi.fn(() => 1),
        invoke: vi.fn().mockResolvedValue(1),
        unregisterCallback: vi.fn(),
      },
    });
    Object.defineProperty(window, "__TAURI_EVENT_PLUGIN_INTERNALS__", {
      configurable: true,
      value: {
        unregisterListener: vi.fn(),
      },
    });
  });

  it("updates visible read totals when stream_counts_updated arrives", async () => {
    render(Layout);

    expect(document.documentElement.style.scrollbarGutter).toBe("auto");
    expect(document.body.style.scrollbarGutter).toBe("auto");
    expect(screen.queryByTestId("connect-toggle-btn")).not.toBeInTheDocument();

    expect(await screen.findByText("0 reads")).toBeInTheDocument();

    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();
    callbacks.onStreamCountsUpdated([
      {
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.1:10000",
        reads_total: 15,
        reads_epoch: 3,
      },
    ]);

    await waitFor(() => {
      expect(screen.getByText("15 reads")).toBeInTheDocument();
    });
  });

  it("keeps forwarder counts server-authored while updating last_read_at", async () => {
    apiMocks.getForwarders.mockResolvedValueOnce({
      forwarders: [
        {
          forwarder_id: "fwd-1",
          display_name: "Forwarder 1",
          online: true,
          readers: [{ reader_ip: "10.0.0.1:10000", connected: true }],
          unique_chips: 3,
          total_reads: 10,
          last_read_at: null,
        },
      ],
    });

    render(Layout);

    await waitFor(() => {
      expect(store.forwarders?.[0]?.total_reads).toBe(10);
    });

    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();

    callbacks.onStreamCountsUpdated([
      {
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.1:10000",
        reads_total: 15,
        reads_epoch: 4,
      },
    ]);

    // Forwarder metrics come from the server snapshot and must not be
    // re-derived from receiver-side stream count events.
    expect(store.forwarders?.[0]?.total_reads).toBe(10);
    expect(store.forwarders?.[0]?.unique_chips).toBe(3);

    callbacks.onLastRead({
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
      chip_id: "chip-4",
      timestamp: "2026-03-21T12:34:56.000Z",
      bib: null,
      name: null,
    });

    expect(store.forwarders?.[0]?.unique_chips).toBe(3);

    // last_read_at should be wall-clock ISO (not the raw reader timestamp)
    const lastReadAt = store.forwarders?.[0]?.last_read_at;
    expect(lastReadAt).toBeTruthy();
    expect(new Date(lastReadAt!).getTime()).not.toBeNaN();
    expect(Math.abs(new Date(lastReadAt!).getTime() - Date.now())).toBeLessThan(
      5000,
    );

    // Same chip again should NOT bump unique_chips
    callbacks.onLastRead({
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
      chip_id: "chip-4",
      timestamp: "2026-03-21T12:35:00.000Z",
      bib: null,
      name: null,
    });
    expect(store.forwarders?.[0]?.unique_chips).toBe(3);
  });

  it("clears stale forwarders on refresh failure so the error state is shown", async () => {
    apiMocks.getForwarders.mockResolvedValueOnce({
      forwarders: [
        {
          forwarder_id: "fwd-1",
          display_name: "Forwarder 1",
          online: true,
          readers: [{ reader_ip: "10.0.0.1:10000", connected: true }],
          unique_chips: 3,
          total_reads: 10,
          last_read_at: null,
        },
      ],
    });

    render(Layout);
    store.activeTab = "forwarders";

    await waitFor(() => {
      expect(screen.getByText("Forwarder 1")).toBeInTheDocument();
    });

    apiMocks.getForwarders.mockRejectedValueOnce(new Error("server offline"));

    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();
    callbacks.onResync();

    await waitFor(() => {
      expect(
        screen.getByText(/Unable to load forwarders:/),
      ).toBeInTheDocument();
    });
    expect(screen.queryByText("Forwarder 1")).not.toBeInTheDocument();
    expect(store.forwarders).toBeNull();
  });

  it("renders nested route content", async () => {
    pageState.pathname = "/admin";
    render(LayoutChildrenHarness);

    expect(await screen.findByTestId("layout-child")).toHaveTextContent(
      "nested route content",
    );
  });
});
