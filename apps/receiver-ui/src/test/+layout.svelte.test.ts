import { render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import Layout from "../routes/+layout.svelte";

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
  getUpdateStatus: vi.fn().mockResolvedValue(null),
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
  putMode: vi.fn().mockResolvedValue(undefined),
  putProfile: vi.fn().mockResolvedValue(undefined),
  checkForUpdate: vi.fn().mockResolvedValue({ status: "up_to_date" }),
  downloadUpdate: vi.fn().mockResolvedValue({ status: "failed" }),
  connect: vi.fn().mockResolvedValue(undefined),
  disconnect: vi.fn().mockResolvedValue(undefined),
  applyUpdate: vi.fn().mockResolvedValue(undefined),
  putEarliestEpoch: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("$lib/api", () => apiMocks);

const sseMocks = vi.hoisted(() => ({
  initSSE: vi.fn(),
  destroySSE: vi.fn(),
}));

vi.mock("$lib/sse", () => ({
  initSSE: sseMocks.initSSE,
  destroySSE: sseMocks.destroySSE,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

describe("receiver layout SSE updates", () => {
  beforeEach(() => {
    vi.clearAllMocks();
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
  });

  it("updates visible read totals when stream_counts_updated arrives", async () => {
    render(Layout);

    expect(document.documentElement.style.scrollbarGutter).toBe("auto");
    expect(document.body.style.scrollbarGutter).toBe("auto");

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
});
