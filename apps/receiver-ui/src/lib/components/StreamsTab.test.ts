import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  getStreams: vi.fn(),
  putSubscriptions: vi.fn(),
  getReplayTargetEpochs: vi.fn().mockResolvedValue({ epochs: [] }),
}));

vi.mock("$lib/api", () => apiMocks);

import StreamsTab from "./StreamsTab.svelte";
import { store, streamKey } from "$lib/store.svelte";

describe("StreamsTab", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    apiMocks.getStreams.mockResolvedValue({
      streams: [],
      degraded: false,
      upstream_error: null,
    });
    apiMocks.putSubscriptions.mockResolvedValue(undefined);
    store.streamActionBusy = false;
    store.streamEventTypeBusy = {};
    store.earliestEpochOptions = {};
    store.earliestEpochLoading = {};
    store.earliestEpochLoadErrors = {};
    store.earliestEpochSaving = {};
    store.targetedEpochInputs = {};
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        disconnect() {}
        unobserve() {}
      },
    );
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      width: 900,
      height: 0,
      top: 0,
      right: 900,
      bottom: 0,
      left: 0,
      x: 0,
      y: 0,
      toJSON() {
        return {};
      },
    });

    const key = streamKey("fwd-1", "10.0.0.1:10000");
    store.modeDraft = "live";
    store.error = null;
    store.dbfEnabled = false;
    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 10100,
          display_alias: "Finish",
          reads_total: 15,
          event_type: undefined,
        },
      ],
      degraded: false,
      upstream_error: null,
    };
    store.lastReads = new Map([
      [
        key,
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          chip_id: "AA:BB:CC:DD",
          timestamp: "2026-03-20T14:23:05.123Z",
          bib: "42",
          name: "Ada Lovelace",
        },
      ],
    ]);
  });

  it("labels subscribed stream rows by reader IP and expanded forwarder details by display name", async () => {
    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "endpoint-1",
          stream_id: "stream-10.0.0.1:10000",
          forwarder_id: "forwarder-internal-id",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 10100,
          display_alias: "Start Line Forwarder",
          reads_total: 15,
        },
      ],
      degraded: false,
      upstream_error: null,
    };

    render(StreamsTab);

    const row = screen.getByText("10.0.0.1:10000").closest("tr")!;
    expect(row).toBeInTheDocument();

    await fireEvent.click(row);

    const forwarderDetail = screen.getByText("Forwarder:").parentElement!;
    expect(forwarderDetail).toHaveTextContent("Start Line Forwarder");
    expect(forwarderDetail).not.toHaveTextContent("forwarder-internal-id");
  });

  it("shows metrics in expanded row when available", async () => {
    const key = streamKey("fwd-1", "10.0.0.1:10000");
    store.streamMetrics = new Map([
      [
        key,
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          raw_count: 1500,
          dedup_count: 1200,
          retransmit_count: 300,
          lag_ms: 2500,
          epoch_raw_count: 500,
          epoch_dedup_count: 400,
          epoch_retransmit_count: 100,
          unique_chips: 75,
          epoch_last_received_at: "2026-03-21T12:00:00Z",
          epoch_lag_ms: 1000,
        },
      ],
    ]);

    render(StreamsTab);

    // Click to expand the row
    const row = screen.getByText("10.0.0.1:10000").closest("tr")!;
    await fireEvent.click(row);

    // Verify lifetime metrics
    expect(screen.getByText("1,500")).toBeInTheDocument(); // raw count
    expect(screen.getByText("1,200")).toBeInTheDocument(); // dedup count
    expect(screen.getByText("300")).toBeInTheDocument(); // retransmit
    expect(screen.getByText("2.5 s")).toBeInTheDocument(); // lag

    // Verify epoch metrics
    expect(screen.getByText("75")).toBeInTheDocument(); // unique chips

    // Verify help text (title attributes)
    expect(
      screen.getByTitle("Total frames received including retransmits"),
    ).toBeInTheDocument();
    expect(
      screen.getByTitle("Distinct chip IDs detected in the current epoch"),
    ).toBeInTheDocument();
  });

  it("shows 'Metrics unavailable' when no metrics data", async () => {
    store.streamMetrics = new Map();

    render(StreamsTab);

    const row = screen.getByText("10.0.0.1:10000").closest("tr")!;
    await fireEvent.click(row);

    expect(screen.getByText("Metrics unavailable")).toBeInTheDocument();
  });

  it("renders last read with time only and left-aligned text", () => {
    render(StreamsTab);

    const timestamp = new Date("2026-03-20T14:23:05.123Z");
    const expectedTime = `${String(timestamp.getHours()).padStart(2, "0")}:${String(timestamp.getMinutes()).padStart(2, "0")}:${String(timestamp.getSeconds()).padStart(2, "0")}.${String(timestamp.getMilliseconds()).padStart(3, "0")}`;
    const lastRead = screen.getByText(
      new RegExp(expectedTime.replace(".", "\\.")),
    );
    expect(lastRead).toBeInTheDocument();
    expect(lastRead).not.toHaveTextContent("2026-03-20");
    expect(lastRead.closest("td")).toHaveClass("text-left");
    expect(lastRead.closest("td")).toHaveClass("w-full");
    expect(screen.getByRole("table")).not.toHaveClass("table-fixed");
    expect(
      screen.getByRole("columnheader", { name: "Stream" }),
    ).not.toHaveClass("w-[120px]");
    expect(screen.getByRole("columnheader", { name: "Stream" })).toHaveClass(
      "w-px",
      "whitespace-nowrap",
    );
  });

  it("renders a bib-only last read as an unknown participant with bib", () => {
    const key = streamKey("fwd-1", "10.0.0.1:10000");
    store.lastReads = new Map([
      [
        key,
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          chip_id: "058000120e38",
          timestamp: "2026-03-20T14:23:05.123Z",
          bib: "1488",
          name: null,
        },
      ],
    ]);

    render(StreamsTab);

    const row = screen.getByText("10.0.0.1:10000").closest("tr")!;
    expect(row).toHaveTextContent("Unknown Participant 1488");
    expect(row).not.toHaveTextContent("Unknown Chip 058000120e38");
  });

  it("renders discovered streams by stream id and subscribes by canonical identity", async () => {
    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "endpoint-abc",
          stream_id: "reader-finish-1",
          forwarder_id: null,
          reader_ip: null,
          subscribed: false,
          local_port: null,
          display_alias: "North Gate",
          stream_epoch: 7,
          reads_total: null,
          reads_epoch: null,
          cursor_epoch: null,
          cursor_seq: null,
        },
      ],
      degraded: false,
      upstream_error: null,
    };
    store.lastReads = new Map();
    store.streamMetrics = new Map();

    render(StreamsTab);

    expect(screen.getByText("reader-finish-1")).toBeInTheDocument();
    expect(screen.getByText("North Gate")).toBeInTheDocument();
    expect(screen.getByText("Available")).toBeInTheDocument();
    expect(screen.getAllByText("—").length).toBeGreaterThan(0);

    store.earliestEpochOptions = {
      "endpoint-abc/reader-finish-1": [
        {
          stream_epoch: 7,
          name: null,
          first_seen_at: null,
          race_names: [],
        },
      ],
    };

    await fireEvent.click(screen.getByText("reader-finish-1").closest("tr")!);
    expect(
      screen.getByTestId("earliest-epoch-endpoint-abc/reader-finish-1"),
    ).toHaveValue("7");
    const subscribe = screen.getByTestId(
      "subscribe-toggle-endpoint-abc/reader-finish-1",
    );
    expect(subscribe).toHaveTextContent("Subscribe");

    await fireEvent.click(subscribe);

    await waitFor(() => {
      expect(apiMocks.putSubscriptions).toHaveBeenCalledWith([
        {
          forwarder_endpoint_id: "endpoint-abc",
          stream_id: "reader-finish-1",
          local_port_override: null,
          event_type: "finish",
        },
      ]);
    });
  });

  it("subscribes all available streams while preserving existing subscriptions", async () => {
    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "endpoint-existing",
          stream_id: "reader-existing",
          forwarder_id: "fwd-existing",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 10100,
          event_type: "start",
        },
        {
          forwarder_endpoint_id: "endpoint-available",
          stream_id: "reader-available",
          forwarder_id: "fwd-available",
          reader_ip: "10.0.0.2:10000",
          subscribed: false,
          local_port: null,
        },
        {
          forwarder_endpoint_id: "endpoint-canonical",
          stream_id: "reader-canonical",
          subscribed: false,
          local_port: null,
        },
      ],
      degraded: false,
      upstream_error: null,
    };

    render(StreamsTab);

    await fireEvent.click(
      screen.getByRole("button", { name: "Subscribe All" }),
    );

    await waitFor(() => {
      expect(apiMocks.putSubscriptions).toHaveBeenCalledWith([
        {
          forwarder_endpoint_id: "endpoint-existing",
          stream_id: "reader-existing",
          forwarder_id: "fwd-existing",
          reader_ip: "10.0.0.1:10000",
          local_port_override: 10100,
          event_type: "start",
        },
        {
          forwarder_endpoint_id: "endpoint-available",
          stream_id: "reader-available",
          forwarder_id: "fwd-available",
          reader_ip: "10.0.0.2:10000",
          local_port_override: null,
          event_type: "finish",
        },
        {
          forwarder_endpoint_id: "endpoint-canonical",
          stream_id: "reader-canonical",
          local_port_override: null,
          event_type: "finish",
        },
      ]);
    });
  });

  it("renders canonical-only streams with distinct identity and expand state", async () => {
    // Two streams with no legacy forwarder_id/reader_ip. Legacy streamKey would
    // collapse both to "/", colliding their each-key and expand slot; canonical
    // identity keeps them distinct.
    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "endpoint-1",
          stream_id: "11111111-1111-1111-1111-111111111111",
          subscribed: true,
          local_port: 10100,
          display_alias: "Alpha",
        },
        {
          forwarder_endpoint_id: "endpoint-2",
          stream_id: "22222222-2222-2222-2222-222222222222",
          subscribed: true,
          local_port: 10200,
          display_alias: "Beta",
        },
      ],
      degraded: false,
      upstream_error: null,
    };

    render(StreamsTab);

    const alphaRow = screen
      .getByText("11111111-1111-1111-1111-111111111111")
      .closest("tr")!;
    const betaRow = screen
      .getByText("22222222-2222-2222-2222-222222222222")
      .closest("tr")!;
    expect(alphaRow).not.toBe(betaRow);

    // Expanding Alpha exposes only Alpha's canonical-keyed controls.
    await fireEvent.click(alphaRow);
    expect(
      screen.getByTestId(
        "subscribe-toggle-endpoint-1/11111111-1111-1111-1111-111111111111",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByTestId(
        "subscribe-toggle-endpoint-2/22222222-2222-2222-2222-222222222222",
      ),
    ).not.toBeInTheDocument();

    // Expanding Beta swaps the expanded slot rather than sharing one.
    await fireEvent.click(betaRow);
    expect(
      screen.getByTestId(
        "subscribe-toggle-endpoint-2/22222222-2222-2222-2222-222222222222",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByTestId(
        "subscribe-toggle-endpoint-1/11111111-1111-1111-1111-111111111111",
      ),
    ).not.toBeInTheDocument();
  });

  it("shows a DBF event type selector when DBF output is enabled", async () => {
    store.dbfEnabled = true;

    render(StreamsTab);

    await fireEvent.click(
      screen.getByRole("button", { name: /10\.0\.0\.1:10000/i }),
    );

    const selector = screen.getByTestId(
      "dbf-event-type-fwd-1/stream-10.0.0.1:10000",
    );
    expect(selector).toBeInTheDocument();
    expect(selector).toHaveValue("finish");
    expect(screen.getByRole("option", { name: "Finish" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Start" })).toBeInTheDocument();
  });
});
