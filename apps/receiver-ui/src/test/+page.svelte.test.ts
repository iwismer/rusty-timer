import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import Page from "../routes/+page.svelte";

const apiMocks = vi.hoisted(() => ({
  getStatus: vi.fn().mockResolvedValue({
    connection_state: "disconnected",
    local_ok: true,
    streams_count: 0,
  }),
  getStreams: vi.fn().mockResolvedValue({
    streams: [],
    degraded: false,
    upstream_error: null,
  }),
  getLogs: vi.fn().mockResolvedValue({ entries: [] }),
  getProfile: vi.fn().mockResolvedValue(null),
  getUpdateStatus: vi.fn().mockResolvedValue(null),
  getSelection: vi.fn().mockResolvedValue({
    selection: { mode: "manual", streams: [] },
    replay_policy: "resume",
  }),
  getRaces: vi.fn().mockResolvedValue({
    races: [{ race_id: "race-1", name: "Race 1", created_at: "2026-01-01" }],
  }),
  getReplayTargetEpochs: vi.fn().mockResolvedValue({
    epochs: [],
  }),
  putSelection: vi.fn().mockResolvedValue(undefined),
  putProfile: vi.fn().mockResolvedValue(undefined),
  checkForUpdate: vi.fn().mockResolvedValue({ status: "up_to_date" }),
  downloadUpdate: vi.fn().mockResolvedValue({ status: "failed" }),
  connect: vi.fn().mockResolvedValue(undefined),
  disconnect: vi.fn().mockResolvedValue(undefined),
  applyUpdate: vi.fn().mockResolvedValue(undefined),
  putSubscriptions: vi.fn().mockResolvedValue(undefined),
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

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("receiver page", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders receiver heading", async () => {
    render(Page);

    expect(
      screen.getByRole("heading", { name: "Receiver" }),
    ).toBeInTheDocument();
  });

  it("renders current epoch number and name in stream rows when available", async () => {
    apiMocks.getStreams.mockResolvedValue({
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: false,
          local_port: null,
          stream_epoch: 12,
          current_epoch_name: "Qualifier",
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    render(Page);

    expect(
      await screen.findByText("epoch: 12 (Qualifier)"),
    ).toBeInTheDocument();
  });

  it("renders current epoch number without name when no epoch name exists", async () => {
    apiMocks.getStreams.mockResolvedValue({
      streams: [
        {
          forwarder_id: "fwd-2",
          reader_ip: "10.0.0.2:10000",
          subscribed: false,
          local_port: null,
          stream_epoch: 4,
          current_epoch_name: null,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    render(Page);

    expect(await screen.findByText("epoch: 4")).toBeInTheDocument();
  });

  it("does not PUT selection when mode changes without clicking Save", async () => {
    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    // Wait a tick so any async side effects would have settled
    await new Promise((r) => setTimeout(r, 50));
    expect(apiMocks.putSelection).not.toHaveBeenCalled();
  });

  it("applies selection mode change after clicking Save", async () => {
    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    const saveBtn = await screen.findByTestId("save-selection-btn");
    await fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(apiMocks.putSelection).toHaveBeenCalledWith(
        expect.objectContaining({
          selection: expect.objectContaining({ mode: "race" }),
        }),
      );
    });
  });

  it("applies race id selection after clicking Save", async () => {
    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    const raceIdSelect = await screen.findByTestId("race-id-select");
    expect(raceIdSelect.tagName).toBe("SELECT");
    await fireEvent.change(raceIdSelect, { target: { value: "race-1" } });

    // Not yet applied
    expect(apiMocks.putSelection).not.toHaveBeenCalled();

    const saveBtn = await screen.findByTestId("save-selection-btn");
    await fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(apiMocks.putSelection).toHaveBeenCalledWith(
        expect.objectContaining({
          selection: expect.objectContaining({
            mode: "race",
            race_id: "race-1",
          }),
        }),
      );
    });
  });

  it("applies epoch scope and replay policy changes after clicking Save", async () => {
    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    const epochScopeSelect = await screen.findByTestId("epoch-scope-select");
    await fireEvent.change(epochScopeSelect, { target: { value: "all" } });
    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "live_only" },
    });

    // Not yet applied
    expect(apiMocks.putSelection).not.toHaveBeenCalled();

    const saveBtn = await screen.findByTestId("save-selection-btn");
    await fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(apiMocks.putSelection).toHaveBeenCalledWith(
        expect.objectContaining({
          selection: expect.objectContaining({
            mode: "race",
            epoch_scope: "all",
          }),
          replay_policy: "live_only",
        }),
      );
    });
    expect(apiMocks.putSelection).toHaveBeenLastCalledWith({
      selection: {
        mode: "race",
        race_id: "",
        epoch_scope: "all",
      },
      replay_policy: "live_only",
    });
  });

  it("shows targeted replay option", async () => {
    render(Page);

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    expect(replayPolicySelect).toHaveTextContent("Resume");
    expect(replayPolicySelect).toHaveTextContent("Live only");
    expect(replayPolicySelect).toHaveTextContent("Targeted replay");
  });

  it("shows plain-language descriptions for epoch scope and replay policy", async () => {
    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    expect(
      await screen.findByText(/Current:\s*replay only the current epoch\./),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/All:\s*replay all epochs available for the race\./),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /Resume:\s*continue from the last acknowledged position\./,
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Live only:\s*skip replay and receive new reads only\./),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /Targeted replay:\s*replay full selected epochs per stream\./,
      ),
    ).toBeInTheDocument();
  });

  it("renders targeted row stream select as dropdown with known stream options", async () => {
    apiMocks.getStreams.mockResolvedValue({
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: false,
          local_port: null,
          display_alias: "Finish",
        },
        {
          forwarder_id: "fwd-2",
          reader_ip: "10.0.0.2:10000",
          subscribed: false,
          local_port: null,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    render(Page);

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "targeted" },
    });

    const streamSelect = await screen.findByTestId("targeted-row-stream-0");
    expect(streamSelect.tagName).toBe("SELECT");
    await waitFor(() => {
      expect(streamSelect).toHaveTextContent("Finish");
      expect(streamSelect).toHaveTextContent("fwd-2 / 10.0.0.2:10000");
    });
  });

  it("shows inline row validation errors and does not submit targeted payload when Save is clicked with invalid rows", async () => {
    render(Page);

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "targeted" },
    });

    const streamSelect = await screen.findByTestId("targeted-row-stream-0");
    await fireEvent.change(streamSelect, { target: { value: "" } });

    const epochSelect = await screen.findByTestId("targeted-row-epoch-0");
    await fireEvent.change(epochSelect, { target: { value: "" } });

    apiMocks.putSelection.mockClear();
    const saveBtn = await screen.findByTestId("save-selection-btn");
    await fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(screen.getByTestId("targeted-row-error-0")).toHaveTextContent(
        "Select a stream",
      );
    });
    expect(screen.getByTestId("targeted-row-error-0")).toHaveTextContent(
      "Stream epoch is required",
    );
    expect(apiMocks.putSelection).not.toHaveBeenCalled();
  });

  it("serializes valid targeted rows into replay_targets payload on Save click", async () => {
    apiMocks.getStreams.mockResolvedValue({
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: false,
          local_port: null,
          display_alias: "Finish",
        },
      ],
      degraded: false,
      upstream_error: null,
    });
    apiMocks.getReplayTargetEpochs.mockResolvedValue({
      epochs: [
        {
          stream_epoch: 3,
          name: "Lap 3",
          first_seen_at: "2026-01-01T10:00:00Z",
          race_names: [],
        },
      ],
    });

    render(Page);

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "targeted" },
    });

    const streamSelect = await screen.findByTestId("targeted-row-stream-0");
    await waitFor(() => {
      expect(streamSelect).toHaveTextContent("Finish");
    });
    await fireEvent.change(streamSelect, {
      target: { value: "fwd-1/10.0.0.1:10000" },
    });

    const epochSelect = await screen.findByTestId("targeted-row-epoch-0");
    await waitFor(() => {
      expect(epochSelect).toHaveTextContent("Lap 3");
    });
    await fireEvent.change(epochSelect, { target: { value: "3" } });

    // Not yet applied
    expect(apiMocks.putSelection).not.toHaveBeenCalled();

    const saveBtn = await screen.findByTestId("save-selection-btn");
    await fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(apiMocks.putSelection).toHaveBeenCalledWith({
        selection: { mode: "manual", streams: [] },
        replay_policy: "targeted",
        replay_targets: [
          {
            forwarder_id: "fwd-1",
            reader_ip: "10.0.0.1:10000",
            stream_epoch: 3,
          },
        ],
      });
    });
  });

  it("adds a targeted replay row when add row is clicked", async () => {
    render(Page);

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "targeted" },
    });

    expect(screen.getByTestId("targeted-row-stream-0")).toBeInTheDocument();
    expect(screen.queryByTestId("targeted-row-stream-1")).toBeNull();

    const addRowButton = await screen.findByTestId("add-targeted-row-btn");
    await fireEvent.click(addRowButton);

    expect(screen.getByTestId("targeted-row-stream-1")).toBeInTheDocument();
    expect(screen.getByTestId("targeted-row-epoch-1")).toBeInTheDocument();
    expect(screen.getByTestId("remove-targeted-row-1")).toBeInTheDocument();
  });

  it("does not render from-seq input for targeted replay rows", async () => {
    render(Page);

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "targeted" },
    });

    expect(screen.queryByTestId("targeted-row-from-seq-0")).toBeNull();
  });

  it("renders targeted row epoch selector as dropdown scoped to selected stream", async () => {
    apiMocks.getStreams.mockResolvedValue({
      streams: [
        {
          stream_id: "stream-1",
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: false,
          local_port: null,
          display_alias: "Finish",
        },
      ],
      degraded: false,
      upstream_error: null,
    });
    apiMocks.getReplayTargetEpochs.mockResolvedValue({
      epochs: [
        {
          stream_epoch: 5,
          name: "Final",
          first_seen_at: "2026-01-02T11:22:33Z",
          race_names: ["Race 1"],
        },
      ],
    });

    render(Page);

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "targeted" },
    });

    const streamSelect = await screen.findByTestId("targeted-row-stream-0");
    await fireEvent.change(streamSelect, {
      target: { value: "fwd-1/10.0.0.1:10000" },
    });

    const epochSelect = await screen.findByTestId("targeted-row-epoch-0");
    expect(epochSelect.tagName).toBe("SELECT");
    await waitFor(() => {
      expect(epochSelect).toHaveTextContent("Final");
      expect(epochSelect).toHaveTextContent("Race 1");
    });
    expect(apiMocks.getReplayTargetEpochs).toHaveBeenCalledWith({
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
    });
  });

  it("uses epoch/timestamp fallback label when epoch name is unavailable", async () => {
    apiMocks.getStreams.mockResolvedValue({
      streams: [
        {
          stream_id: "stream-2",
          forwarder_id: "fwd-2",
          reader_ip: "10.0.0.2:10000",
          subscribed: false,
          local_port: null,
        },
      ],
      degraded: false,
      upstream_error: null,
    });
    apiMocks.getReplayTargetEpochs.mockResolvedValue({
      epochs: [
        {
          stream_epoch: 9,
          name: null,
          first_seen_at: "2026-01-03T00:00:00Z",
          race_names: [],
        },
      ],
    });

    render(Page);

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "targeted" },
    });

    const streamSelect = await screen.findByTestId("targeted-row-stream-0");
    await fireEvent.change(streamSelect, {
      target: { value: "fwd-2/10.0.0.2:10000" },
    });

    const epochSelect = await screen.findByTestId("targeted-row-epoch-0");
    await waitFor(() => {
      expect(epochSelect).toHaveTextContent("Epoch 9");
      expect(epochSelect).toHaveTextContent("2026");
    });
  });

  it("retries epoch fetch for a stream after transient failure", async () => {
    apiMocks.getStreams.mockResolvedValue({
      streams: [
        {
          stream_id: "stream-2",
          forwarder_id: "fwd-2",
          reader_ip: "10.0.0.2:10000",
          subscribed: false,
          local_port: null,
        },
      ],
      degraded: false,
      upstream_error: null,
    });
    apiMocks.getReplayTargetEpochs
      .mockRejectedValueOnce(new Error("temporary upstream failure"))
      .mockResolvedValueOnce({
        epochs: [
          {
            stream_epoch: 11,
            name: "Lap 11",
            first_seen_at: "2026-01-03T00:00:00Z",
            race_names: [],
          },
        ],
      });

    render(Page);

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "targeted" },
    });

    const streamSelect = await screen.findByTestId("targeted-row-stream-0");
    await fireEvent.change(streamSelect, {
      target: { value: "fwd-2/10.0.0.2:10000" },
    });
    await fireEvent.change(streamSelect, { target: { value: "" } });
    await fireEvent.change(streamSelect, {
      target: { value: "fwd-2/10.0.0.2:10000" },
    });

    await waitFor(() => {
      expect(apiMocks.getReplayTargetEpochs).toHaveBeenCalledTimes(2);
    });

    const epochSelect = await screen.findByTestId("targeted-row-epoch-0");
    await waitFor(() => {
      expect(epochSelect).toHaveTextContent("Lap 11");
    });
  });

  it("removes a targeted replay row when remove is clicked", async () => {
    render(Page);

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "targeted" },
    });

    const addRowButton = await screen.findByTestId("add-targeted-row-btn");
    await fireEvent.click(addRowButton);
    expect(screen.getByTestId("targeted-row-stream-1")).toBeInTheDocument();

    const removeSecondRowButton = await screen.findByTestId(
      "remove-targeted-row-1",
    );
    await fireEvent.click(removeSecondRowButton);

    await waitFor(() => {
      expect(screen.queryByTestId("targeted-row-stream-1")).toBeNull();
    });
    expect(screen.getByTestId("targeted-row-stream-0")).toBeInTheDocument();
  });

  it("coalesces rapid Save clicks so the latest payload wins", async () => {
    const firstApply = deferred<void>();
    apiMocks.putSelection.mockReset();
    apiMocks.putSelection
      .mockReturnValueOnce(firstApply.promise)
      .mockResolvedValue(undefined);

    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    const saveBtn = await screen.findByTestId("save-selection-btn");
    await fireEvent.click(saveBtn);

    // While first save is in-flight, change replay policy and click Save again
    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "live_only" },
    });
    await fireEvent.click(saveBtn);

    firstApply.resolve();

    await waitFor(() => {
      expect(apiMocks.putSelection).toHaveBeenCalledTimes(2);
    });
    expect(apiMocks.putSelection).toHaveBeenLastCalledWith({
      selection: {
        mode: "race",
        race_id: "",
        epoch_scope: "current",
      },
      replay_policy: "live_only",
    });
  });

  it("retries queued Save after in-flight request fails", async () => {
    const firstApply = deferred<void>();
    apiMocks.putSelection.mockReset();
    apiMocks.putSelection
      .mockReturnValueOnce(firstApply.promise)
      .mockResolvedValue(undefined);

    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    const saveBtn = await screen.findByTestId("save-selection-btn");
    await fireEvent.click(saveBtn);

    // While first save is in-flight, change replay policy and click Save again
    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "live_only" },
    });
    await fireEvent.click(saveBtn);

    firstApply.reject(new Error("network error"));

    await waitFor(() => {
      expect(apiMocks.putSelection).toHaveBeenCalledTimes(2);
    });
    expect(apiMocks.putSelection).toHaveBeenLastCalledWith({
      selection: {
        mode: "race",
        race_id: "",
        epoch_scope: "current",
      },
      replay_policy: "live_only",
    });
  });

  it("refreshes streams list when stream count update references an unknown stream", async () => {
    apiMocks.getStreams.mockReset();
    apiMocks.getStreams
      .mockResolvedValueOnce({
        streams: [
          {
            forwarder_id: "fwd-1",
            reader_ip: "10.0.0.1:10000",
            subscribed: false,
            local_port: null,
          },
        ],
        degraded: false,
        upstream_error: null,
      })
      .mockResolvedValueOnce({
        streams: [
          {
            forwarder_id: "fwd-1",
            reader_ip: "10.0.0.1:10000",
            subscribed: false,
            local_port: null,
          },
          {
            forwarder_id: "fwd-2",
            reader_ip: "10.0.0.2:10000",
            subscribed: false,
            local_port: null,
          },
        ],
        degraded: false,
        upstream_error: null,
      });

    render(Page);

    await waitFor(() => {
      expect(screen.getByTestId("streams-section")).toHaveTextContent(
        "fwd-1 / 10.0.0.1:10000",
      );
    });
    expect(screen.getByTestId("streams-section")).not.toHaveTextContent(
      "fwd-2 / 10.0.0.2:10000",
    );

    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();
    callbacks.onStreamCountsUpdated([
      {
        forwarder_id: "fwd-2",
        reader_ip: "10.0.0.2:10000",
        reads_total: 1,
        reads_epoch: 1,
      },
    ]);

    await waitFor(() => {
      expect(screen.getByTestId("streams-section")).toHaveTextContent(
        "fwd-2 / 10.0.0.2:10000",
      );
    });
  });

  it("coalesces burst unknown-stream updates into a single in-flight full reload", async () => {
    apiMocks.getStreams.mockResolvedValue({
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: false,
          local_port: null,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    render(Page);

    await waitFor(() => {
      expect(screen.getByTestId("streams-section")).toHaveTextContent(
        "fwd-1 / 10.0.0.1:10000",
      );
    });

    const statusResync = deferred<{
      connection_state: string;
      local_ok: boolean;
      streams_count: number;
    }>();
    apiMocks.getStatus.mockReset();
    apiMocks.getStatus
      .mockReturnValueOnce(statusResync.promise)
      .mockResolvedValue({
        connection_state: "connected",
        local_ok: true,
        streams_count: 2,
      });
    apiMocks.getStreams.mockReset();
    apiMocks.getStreams.mockResolvedValue({
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: false,
          local_port: null,
        },
        {
          forwarder_id: "fwd-2",
          reader_ip: "10.0.0.2:10000",
          subscribed: false,
          local_port: null,
        },
      ],
      degraded: false,
      upstream_error: null,
    });
    apiMocks.getLogs.mockReset();
    apiMocks.getLogs.mockResolvedValue({ entries: [] });
    apiMocks.getSelection.mockReset();
    apiMocks.getSelection.mockResolvedValue({
      selection: { mode: "manual", streams: [] },
      replay_policy: "resume",
    });
    apiMocks.getRaces.mockReset();
    apiMocks.getRaces.mockResolvedValue({ races: [] });

    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();
    callbacks.onStreamCountsUpdated([
      {
        forwarder_id: "fwd-2",
        reader_ip: "10.0.0.2:10000",
        reads_total: 1,
        reads_epoch: 1,
      },
    ]);
    callbacks.onStreamCountsUpdated([
      {
        forwarder_id: "fwd-3",
        reader_ip: "10.0.0.3:10000",
        reads_total: 1,
        reads_epoch: 1,
      },
    ]);

    expect(apiMocks.getStatus).toHaveBeenCalledTimes(1);
    statusResync.resolve({
      connection_state: "connected",
      local_ok: true,
      streams_count: 2,
    });

    await waitFor(() => {
      expect(screen.getByTestId("streams-section")).toHaveTextContent(
        "fwd-2 / 10.0.0.2:10000",
      );
    });
  });

  it("patches counts for known streams without triggering a full reload", async () => {
    apiMocks.getStreams.mockResolvedValue({
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: false,
          local_port: null,
          reads_total: 10,
          reads_epoch: 4,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    render(Page);

    await waitFor(() => {
      expect(screen.getByTestId("streams-section")).toHaveTextContent(
        "fwd-1 / 10.0.0.1:10000",
      );
    });

    apiMocks.getStatus.mockClear();
    apiMocks.getStreams.mockClear();
    apiMocks.getLogs.mockClear();

    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();
    callbacks.onStreamCountsUpdated([
      {
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.1:10000",
        reads_total: 11,
        reads_epoch: 5,
      },
    ]);

    await waitFor(() => {
      expect(apiMocks.getStatus).not.toHaveBeenCalled();
      expect(apiMocks.getStreams).not.toHaveBeenCalled();
      expect(apiMocks.getLogs).not.toHaveBeenCalled();
    });
  });

  it("shows AlertBanner when Save click triggers putSelection failure", async () => {
    const errMsg = "server rejected selection: 422";
    apiMocks.putSelection.mockRejectedValueOnce(new Error(errMsg));

    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    const saveBtn = await screen.findByTestId("save-selection-btn");
    await fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(screen.getByText(`Error: ${errMsg}`)).toBeInTheDocument();
    });
  });

  // --- Save button state tests ---

  it("Save button is disabled on initial load when local matches server", async () => {
    render(Page);

    const saveBtn = await screen.findByTestId("save-selection-btn");
    await waitFor(() => {
      expect(saveBtn).toBeDisabled();
    });
  });

  it("Save button becomes enabled after changing a control", async () => {
    render(Page);

    const saveBtn = await screen.findByTestId("save-selection-btn");
    await waitFor(() => {
      expect(saveBtn).toBeDisabled();
    });

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    await waitFor(() => {
      expect(saveBtn).toBeEnabled();
    });
  });

  it("Save button returns to disabled after successful save", async () => {
    render(Page);

    const saveBtn = await screen.findByTestId("save-selection-btn");

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    await waitFor(() => {
      expect(saveBtn).toBeEnabled();
    });

    await fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(apiMocks.putSelection).toHaveBeenCalled();
    });

    await waitFor(() => {
      expect(saveBtn).toBeDisabled();
    });
  });

  it("Save button is disabled while save is in-flight", async () => {
    const applyDeferred = deferred<void>();
    apiMocks.putSelection.mockReset();
    apiMocks.putSelection.mockReturnValue(applyDeferred.promise);

    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    const saveBtn = await screen.findByTestId("save-selection-btn");
    await waitFor(() => {
      expect(saveBtn).toBeEnabled();
    });

    await fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(saveBtn).toBeDisabled();
    });
    expect(saveBtn).toHaveTextContent("Saving...");

    applyDeferred.resolve();

    await waitFor(() => {
      expect(saveBtn).toHaveTextContent("Save");
    });
  });

  it("Save button becomes enabled after changing replay policy", async () => {
    render(Page);

    const saveBtn = await screen.findByTestId("save-selection-btn");
    await waitFor(() => {
      expect(saveBtn).toBeDisabled();
    });

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "live_only" },
    });

    await waitFor(() => {
      expect(saveBtn).toBeEnabled();
    });
  });
});
