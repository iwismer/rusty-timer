import { render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import Layout from "../routes/+layout.svelte";
import LayoutChildrenHarness from "./LayoutChildrenHarness.svelte";

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

  it("renders nested route content", async () => {
    pageState.pathname = "/admin";
    render(LayoutChildrenHarness);

    expect(await screen.findByTestId("layout-child")).toHaveTextContent(
      "nested route content",
    );
  });
});
