import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  getStreams: vi.fn(),
  putSubscriptions: vi.fn(),
  getStreamEpochs: vi.fn().mockResolvedValue({ epochs: [] }),
  updateSubscriptionEventType: vi.fn().mockResolvedValue(undefined),
  readerAdvanceEpoch: vi.fn().mockResolvedValue({ success: true }),
  readerSyncClock: vi.fn().mockResolvedValue({ success: true }),
  readerClearReads: vi.fn().mockResolvedValue({ success: true }),
  setStreamAnnouncerPublish: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("$lib/api", () => apiMocks);

import StreamsTab from "./StreamsTab.svelte";
import { store, setStreamRole } from "$lib/store.svelte";

describe("StreamsTab Race Day Enhancements", () => {
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
    store.streamSubscriptionPendingSince = {};
    store.streamActivityAt = new Map();
    store.streamEventTypeBusy = {};
    store.streamEpochOptions = {};
    store.streamRoles = {};
    store.streamCourseOrder = [];
    store.connections = {
      server: {
        configured: true,
        endpoint_id: "srv-1",
        server_address: "https://server.example.com",
        reachable: true,
        approval_state: "active",
        waiting_for_approval: false,
        message: null,
      },
      forwarders: [
        {
          endpoint_id: "fwd-1",
          display_name: "box-01",
          state: "subscribed",
          pending: false,
          subscribed_count: 2,
          available_count: 2,
          last_seen_secs: 0,
          ups: null,
          restart_needed: false,
          remote_config_available: false,
          readers: [
            {
              stream_id: "reader-100",
              connected: true,
              state: "connected",
              last_read_unix_ms: null,
              hardware_reader_id: null,
              firmware_version: null,
              model: null,
              reader_info: {
                clock: {
                  drift_ms: 1,
                  reader_clock: "2026-10-25T12:00:00Z",
                },
              },
            },
            {
              stream_id: "reader-101",
              connected: true,
              state: "connected",
              last_read_unix_ms: null,
              hardware_reader_id: null,
              firmware_version: null,
              model: null,
              reader_info: {
                clock: {
                  drift_ms: 1420,
                  reader_clock: "2026-10-25T12:00:00Z",
                },
              },
            },
          ],
        },
      ],
    };

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

    store.modeDraft = "live";
    store.error = null;
    store.dbfEnabled = true;
    store.streams = {
      streams: [
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "reader-100",
          forwarder_id: "box-01",
          reader_ip: "192.168.1.100",
          subscribed: true,
          local_port: 10000,
          event_type: "start",
          stream_epoch: 1,
          current_epoch_name: "5K Mass Start",
          reads_epoch: 540,
          reads_total: 1440,
        },
        {
          forwarder_endpoint_id: "fwd-1",
          stream_id: "reader-101",
          forwarder_id: "box-01",
          reader_ip: "192.168.1.101",
          subscribed: true,
          local_port: 10001,
          event_type: "start",
          stream_epoch: 1,
          current_epoch_name: "5K Mass Start",
          reads_epoch: 530,
          reads_total: 1431,
        },
      ],
      degraded: false,
      upstream_error: null,
    };
  });

  it("renders reader drift chip and prominent epoch reads with correct color thresholds", () => {
    // reader-100: 1ms (<250ms -> green ok)
    // reader-101: 1420ms (>=1000ms -> red err)
    render(StreamsTab);

    const chip1 = screen.getByText("+1ms");
    expect(chip1).toBeInTheDocument();
    expect(chip1.className).toContain("text-status-ok");

    const chip2 = screen.getByText("+1420ms");
    expect(chip2).toBeInTheDocument();
    expect(chip2.className).toContain("text-status-err");

    // Prominent epoch reads: 540 and 530
    expect(screen.getByText("540")).toBeInTheDocument();
    expect(screen.getByText("530")).toBeInTheDocument();
  });

  it("styles drift chip with green ok for drift between 100ms and 249ms", () => {
    store.connections!.forwarders[0].readers[0].reader_info!.clock!.drift_ms = 150;
    render(StreamsTab);

    const chip = screen.getByText("+150ms");
    expect(chip).toBeInTheDocument();
    expect(chip.className).toContain("text-status-ok");
  });

  it("shows Start / Finish active mode badge and cutover button when in start mode", async () => {
    // Designate reader-100 and reader-101 as start_finish
    setStreamRole(store.streams!.streams[0], "start_finish");
    setStreamRole(store.streams!.streams[1], "start_finish");

    render(StreamsTab);

    // Active mode in badge shows START
    const badges = screen.getAllByText(/Start \/ Finish\s*·\s*START/i);
    expect(badges).toHaveLength(2);

    // Cutover button is present with count 2
    const cutoverBtn = screen.getByTestId("advance-starts-to-finish-btn");
    expect(cutoverBtn).toBeInTheDocument();
    expect(cutoverBtn).toHaveTextContent("Advance Starts to Finish");
    expect(cutoverBtn).toHaveTextContent("2");

    // Clicking cutover opens confirmation
    await fireEvent.click(cutoverBtn);
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    // Confirm cutover
    const confirmBtn = screen.getByRole("button", {
      name: "Advance to Finish",
    });
    await fireEvent.click(confirmBtn);

    await waitFor(() => {
      // Both streams should have called readerAdvanceEpoch and updateSubscriptionEventType to finish
      expect(apiMocks.readerAdvanceEpoch).toHaveBeenCalledTimes(2);
      expect(apiMocks.updateSubscriptionEventType).toHaveBeenCalledWith(
        { forwarder_endpoint_id: "fwd-1", stream_id: "reader-100" },
        "finish",
      );
      expect(apiMocks.updateSubscriptionEventType).toHaveBeenCalledWith(
        { forwarder_endpoint_id: "fwd-1", stream_id: "reader-101" },
        "finish",
      );
    });
  });

  it("allows inline epoch advancement with custom name in expanded row", async () => {
    render(StreamsTab);

    // Click row 1 to expand
    const row = screen.getByText("192.168.1.100").closest("tr")!;
    await fireEvent.click(row);

    const input = screen.getByTestId("epoch-name-input-fwd-1/reader-100");
    expect(input).toBeInTheDocument();
    await fireEvent.input(input, { target: { value: "Wave 2 Start" } });

    const advanceBtn = screen.getByTestId("advance-epoch-btn-fwd-1/reader-100");
    await fireEvent.click(advanceBtn);

    await waitFor(() => {
      expect(apiMocks.readerAdvanceEpoch).toHaveBeenCalledWith(
        "fwd-1",
        "reader-100",
        "Wave 2 Start",
      );
    });
  });

  it("supports single-reader clock sync in expanded row", async () => {
    render(StreamsTab);

    const row = screen.getByText("192.168.1.100").closest("tr")!;
    await fireEvent.click(row);

    const syncBtn = screen.getByTestId("sync-clock-fwd-1/reader-100");
    await fireEvent.click(syncBtn);

    await waitFor(() => {
      expect(apiMocks.readerSyncClock).toHaveBeenCalledWith(
        "fwd-1",
        "reader-100",
      );
    });
  });

  it("supports reordering streams with move up and move down buttons", async () => {
    render(StreamsTab);

    const moveDownButtons = screen.getAllByTitle("Move later in course order");
    // First reader has move down enabled
    expect(moveDownButtons[0]).not.toBeDisabled();

    // Click move down on the first reader
    await fireEvent.click(moveDownButtons[0]);

    // Check store.streamCourseOrder updated
    expect(store.streamCourseOrder).toEqual([
      "fwd-1/reader-101",
      "fwd-1/reader-100",
    ]);
  });

  it("renders forwarder configuration button in expanded row", async () => {
    render(StreamsTab);

    // Click row 1 to expand
    const row = screen.getByText("192.168.1.100").closest("tr")!;
    await fireEvent.click(row);

    const configBtn = screen.getByTestId(
      "stream-fwd-config-btn-fwd-1/reader-100",
    );
    expect(configBtn).toBeInTheDocument();
  });
});
