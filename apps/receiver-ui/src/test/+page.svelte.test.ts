import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import Page from "../routes/+page.svelte";

const fixtures = vi.hoisted(() => ({
  activeStreamsResponse: {
    streams: [
      {
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.1:10000",
        subscribed: false,
        local_port: null,
        stream_epoch: 5,
        paused: false,
      },
    ],
    degraded: false,
    upstream_error: null,
  },
  pausedStreamsResponse: {
    streams: [
      {
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.1:10000",
        subscribed: false,
        local_port: null,
        stream_epoch: 5,
        paused: true,
      },
    ],
    degraded: false,
    upstream_error: null,
  },
}));

const apiMocks = vi.hoisted(() => ({
  getStatus: vi.fn().mockResolvedValue({
    connection_state: "disconnected",
    local_ok: true,
    streams_count: 1,
  }),
  getStreams: vi.fn().mockResolvedValue(fixtures.activeStreamsResponse),
  getLogs: vi.fn().mockResolvedValue({ entries: [] }),
  getProfile: vi.fn().mockResolvedValue(null),
  getUpdateStatus: vi.fn().mockResolvedValue(null),
  getMode: vi.fn().mockResolvedValue({
    mode: "live",
    streams: [],
    earliest_epochs: [],
  }),
  getRaces: vi.fn().mockResolvedValue({
    races: [{ race_id: "race-1", name: "Race 1", created_at: "2026-01-01" }],
  }),
  getReplayTargetEpochs: vi.fn().mockResolvedValue({
    epochs: [
      { stream_epoch: 9, name: "Open", first_seen_at: null, race_names: [] },
      { stream_epoch: 5, name: "Main", first_seen_at: null, race_names: [] },
      { stream_epoch: 3, name: null, first_seen_at: null, race_names: [] },
    ],
  }),
  putMode: vi.fn().mockResolvedValue(undefined),
  putProfile: vi.fn().mockResolvedValue(undefined),
  checkForUpdate: vi.fn().mockResolvedValue({ status: "up_to_date" }),
  downloadUpdate: vi.fn().mockResolvedValue({ status: "failed" }),
  connect: vi.fn().mockResolvedValue(undefined),
  disconnect: vi.fn().mockResolvedValue(undefined),
  applyUpdate: vi.fn().mockResolvedValue(undefined),
  pauseStream: vi.fn().mockResolvedValue(undefined),
  resumeStream: vi.fn().mockResolvedValue(undefined),
  pauseAll: vi.fn().mockResolvedValue(undefined),
  resumeAll: vi.fn().mockResolvedValue(undefined),
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

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("receiver page mode controls", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders receiver heading", async () => {
    render(Page);

    expect(
      screen.getByRole("heading", { name: "Receiver" }),
    ).toBeInTheDocument();
  });

  it("loadAll fetches mode and hydrates race state", async () => {
    apiMocks.getMode.mockResolvedValueOnce({ mode: "race", race_id: "race-1" });

    render(Page);

    const modeSelect = await screen.findByTestId("mode-select");
    expect((modeSelect as HTMLSelectElement).value).toBe("race");

    const raceSelect = await screen.findByTestId("race-id-select");
    expect((raceSelect as HTMLSelectElement).value).toBe("race-1");
  });

  it("applies live mode with all available streams on Apply Mode", async () => {
    apiMocks.getStreams.mockResolvedValueOnce({
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: false,
          local_port: null,
          stream_epoch: 5,
          paused: false,
        },
        {
          forwarder_id: "fwd-2",
          reader_ip: "10.0.0.2:10000",
          subscribed: false,
          local_port: null,
          stream_epoch: 7,
          paused: false,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    render(Page);

    const apply = await screen.findByTestId("save-mode-btn");
    await waitFor(() => {
      expect(apply).not.toBeDisabled();
    });
    await fireEvent.click(apply);

    await waitFor(() => {
      expect(apiMocks.putMode).toHaveBeenCalledWith({
        mode: "live",
        streams: [
          { forwarder_id: "fwd-1", reader_ip: "10.0.0.1:10000" },
          { forwarder_id: "fwd-2", reader_ip: "10.0.0.2:10000" },
        ],
        earliest_epochs: [],
      });
    });
  });

  it("does not render live mode include checkbox", async () => {
    render(Page);

    await screen.findByTestId("earliest-epoch-fwd-1/10.0.0.1:10000");

    expect(
      screen.queryByTestId("live-stream-toggle-fwd-1/10.0.0.1:10000"),
    ).not.toBeInTheDocument();
  });

  it("reacts to ModeChanged SSE events", async () => {
    render(Page);

    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();

    callbacks.onModeChanged({ mode: "targeted_replay", targets: [] });

    const modeSelect = await screen.findByTestId("mode-select");
    expect((modeSelect as HTMLSelectElement).value).toBe("targeted_replay");
  });

  it("pauses stream in live mode", async () => {
    render(Page);

    const button = await screen.findByTestId(
      "pause-resume-fwd-1/10.0.0.1:10000",
    );
    await fireEvent.click(button);

    await waitFor(() => {
      expect(apiMocks.pauseStream).toHaveBeenCalledWith({
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.1:10000",
      });
    });
  });

  it("triggers Pause All in live mode", async () => {
    render(Page);

    const button = await screen.findByTestId("pause-all-btn");
    await fireEvent.click(button);

    await waitFor(() => {
      expect(apiMocks.pauseAll).toHaveBeenCalled();
    });
  });

  it("prefetches epoch options for loaded streams", async () => {
    render(Page);

    await waitFor(() => {
      expect(apiMocks.getReplayTargetEpochs).toHaveBeenCalledWith({
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.1:10000",
      });
    });
  });

  it("styles Pause All as warn and Resume All as ok", async () => {
    render(Page);

    const pauseAll = await screen.findByTestId("pause-all-btn");
    const resumeAll = await screen.findByTestId("resume-all-btn");

    expect(pauseAll.className).toContain("bg-status-warn-bg");
    expect(resumeAll.className).toContain("bg-status-ok-bg");
  });

  it("disables Resume All when all streams are already resumed", async () => {
    render(Page);

    const pauseAll = await screen.findByTestId("pause-all-btn");
    const resumeAll = await screen.findByTestId("resume-all-btn");

    expect(pauseAll).not.toBeDisabled();
    expect(resumeAll).toBeDisabled();
  });

  it("disables Pause All when all streams are already paused", async () => {
    apiMocks.getStreams.mockResolvedValueOnce(fixtures.pausedStreamsResponse);
    render(Page);

    const pauseAll = await screen.findByTestId("pause-all-btn");
    const resumeAll = await screen.findByTestId("resume-all-btn");

    expect(pauseAll).toBeDisabled();
    expect(resumeAll).not.toBeDisabled();
  });

  it("styles per-row Pause as warn and Resume as ok", async () => {
    render(Page);
    const activeRowButton = await screen.findByTestId(
      "pause-resume-fwd-1/10.0.0.1:10000",
    );
    expect(activeRowButton).toHaveTextContent("Pause");
    expect(activeRowButton.className).toContain("bg-status-warn-bg");

    apiMocks.getStreams.mockResolvedValueOnce(fixtures.pausedStreamsResponse);
    await fireEvent.click(activeRowButton);

    await waitFor(() => {
      expect(activeRowButton).toHaveTextContent("Resume");
    });
    expect(activeRowButton.className).toContain("bg-status-ok-bg");
  });

  it("prevents overlapping pause/resume stream actions while one is in flight", async () => {
    const pauseDeferred = deferred<void>();
    apiMocks.pauseStream.mockImplementationOnce(() => pauseDeferred.promise);

    render(Page);

    const button = await screen.findByTestId(
      "pause-resume-fwd-1/10.0.0.1:10000",
    );
    await fireEvent.click(button);
    await fireEvent.click(button);

    expect(apiMocks.pauseStream).toHaveBeenCalledTimes(1);
    expect(button).toBeDisabled();

    pauseDeferred.resolve();

    await waitFor(() => {
      expect(button).not.toBeDisabled();
    });
  });

  it("prevents overlapping pause/resume all actions while one is in flight", async () => {
    const pauseAllDeferred = deferred<void>();
    apiMocks.pauseAll.mockImplementationOnce(() => pauseAllDeferred.promise);

    render(Page);

    const pauseAll = await screen.findByTestId("pause-all-btn");
    const resumeAll = await screen.findByTestId("resume-all-btn");

    await fireEvent.click(pauseAll);
    await fireEvent.click(resumeAll);

    expect(apiMocks.pauseAll).toHaveBeenCalledTimes(1);
    expect(apiMocks.resumeAll).not.toHaveBeenCalled();
    expect(pauseAll).toBeDisabled();
    expect(resumeAll).toBeDisabled();

    pauseAllDeferred.resolve();

    await waitFor(() => {
      expect(pauseAll).not.toBeDisabled();
      expect(resumeAll).toBeDisabled();
    });
  });

  it("does not clobber unsaved local mode edits when loadAll hydration resolves", async () => {
    const firstHydrationDeferred = deferred<{
      mode: "race";
      race_id: string;
    }>();
    apiMocks.getMode.mockImplementationOnce(
      () => firstHydrationDeferred.promise,
    );

    render(Page);

    const modeSelect = await screen.findByTestId("mode-select");

    await fireEvent.change(modeSelect, {
      target: { value: "targeted_replay" },
    });

    firstHydrationDeferred.resolve({ mode: "race", race_id: "race-1" });

    await waitFor(() => {
      expect((modeSelect as HTMLSelectElement).value).toBe("targeted_replay");
    });
    expect(screen.queryByTestId("race-id-select")).not.toBeInTheDocument();
  });

  it("ignores stale loadAll snapshot started during pause-all action", async () => {
    const staleLoadDeferred = deferred<typeof fixtures.activeStreamsResponse>();
    const pauseAllDeferred = deferred<void>();

    apiMocks.pauseAll.mockImplementationOnce(() => pauseAllDeferred.promise);
    apiMocks.getStreams
      .mockResolvedValueOnce(fixtures.activeStreamsResponse)
      .mockImplementationOnce(() => staleLoadDeferred.promise)
      .mockResolvedValueOnce(fixtures.pausedStreamsResponse);

    render(Page);

    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();

    const pauseAll = await screen.findByTestId("pause-all-btn");
    const streamButton = await screen.findByTestId(
      "pause-resume-fwd-1/10.0.0.1:10000",
    );
    expect(streamButton).toHaveTextContent("Pause");

    await fireEvent.click(pauseAll);
    callbacks.onResync();
    pauseAllDeferred.resolve();

    await waitFor(() => {
      expect(apiMocks.pauseAll).toHaveBeenCalledTimes(1);
      expect(streamButton).toHaveTextContent("Resume");
    });

    staleLoadDeferred.resolve(fixtures.activeStreamsResponse);

    await waitFor(() => {
      expect(streamButton).toHaveTextContent("Resume");
    });
  });

  it("does not let stale loadAll mode hydration overwrite a newer applied mode", async () => {
    const staleModeDeferred = deferred<{
      mode: "live";
      streams: [];
      earliest_epochs: [];
    }>();
    apiMocks.getMode
      .mockResolvedValueOnce({
        mode: "live",
        streams: [],
        earliest_epochs: [],
      })
      .mockImplementationOnce(() => staleModeDeferred.promise);

    render(Page);

    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();

    const modeSelect = await screen.findByTestId("mode-select");
    await fireEvent.change(modeSelect, {
      target: { value: "targeted_replay" },
    });
    callbacks.onResync();
    await waitFor(() => {
      expect(apiMocks.getMode).toHaveBeenCalledTimes(2);
    });

    const apply = await screen.findByTestId("save-mode-btn");
    await fireEvent.click(apply);

    await waitFor(() => {
      expect(apiMocks.putMode).toHaveBeenCalledWith({
        mode: "targeted_replay",
        targets: [],
      });
      expect((modeSelect as HTMLSelectElement).value).toBe("targeted_replay");
    });

    staleModeDeferred.resolve({
      mode: "live",
      streams: [],
      earliest_epochs: [],
    });

    await waitFor(() => {
      expect((modeSelect as HTMLSelectElement).value).toBe("targeted_replay");
    });
  });

  it("does not let stale loadAll streams overwrite a newer SSE snapshot", async () => {
    const staleStreamsDeferred =
      deferred<typeof fixtures.activeStreamsResponse>();
    apiMocks.getStreams
      .mockResolvedValueOnce(fixtures.activeStreamsResponse)
      .mockImplementationOnce(() => staleStreamsDeferred.promise);

    render(Page);

    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();

    const streamButton = await screen.findByTestId(
      "pause-resume-fwd-1/10.0.0.1:10000",
    );
    expect(streamButton).toHaveTextContent("Pause");

    callbacks.onResync();
    await waitFor(() => {
      expect(apiMocks.getStreams).toHaveBeenCalledTimes(2);
    });
    callbacks.onStreamsSnapshot(fixtures.pausedStreamsResponse);

    await waitFor(() => {
      expect(streamButton).toHaveTextContent("Resume");
    });

    staleStreamsDeferred.resolve(fixtures.activeStreamsResponse);

    await waitFor(() => {
      expect(streamButton).toHaveTextContent("Resume");
    });
  });

  it("releases stream action busy state when pause stream rejects", async () => {
    const rejection = new Error("pause failed");
    apiMocks.pauseStream.mockRejectedValueOnce(rejection);

    render(Page);

    const button = await screen.findByTestId(
      "pause-resume-fwd-1/10.0.0.1:10000",
    );
    await fireEvent.click(button);

    await waitFor(() => {
      expect(apiMocks.pauseStream).toHaveBeenCalledTimes(1);
      expect(button).not.toBeDisabled();
    });
    expect(screen.getByText(String(rejection))).toBeInTheDocument();
  });

  it("releases stream action busy state when pause-all rejects", async () => {
    const rejection = new Error("pause-all failed");
    apiMocks.pauseAll.mockRejectedValueOnce(rejection);

    render(Page);

    const pauseAll = await screen.findByTestId("pause-all-btn");
    const resumeAll = await screen.findByTestId("resume-all-btn");
    await fireEvent.click(pauseAll);

    await waitFor(() => {
      expect(apiMocks.pauseAll).toHaveBeenCalledTimes(1);
      expect(pauseAll).not.toBeDisabled();
      expect(resumeAll).toBeDisabled();
    });
    expect(screen.getByText(String(rejection))).toBeInTheDocument();
  });

  it("replaces set earliest button with a dropdown", async () => {
    render(Page);

    const earliest = await screen.findByTestId(
      "earliest-epoch-fwd-1/10.0.0.1:10000",
    );

    expect(earliest.tagName).toBe("SELECT");
    expect(
      screen.queryByTestId("apply-earliest-fwd-1/10.0.0.1:10000"),
    ).not.toBeInTheDocument();
  });

  it("defaults earliest dropdown to current stream epoch when available", async () => {
    render(Page);

    const earliest = await screen.findByTestId(
      "earliest-epoch-fwd-1/10.0.0.1:10000",
    );

    await waitFor(() => {
      expect((earliest as HTMLSelectElement).value).toBe("5");
    });
  });

  it("updates earliest epoch immediately when dropdown changes", async () => {
    render(Page);

    const earliest = await screen.findByTestId(
      "earliest-epoch-fwd-1/10.0.0.1:10000",
    );
    await fireEvent.change(earliest, { target: { value: "3" } });

    await waitFor(() => {
      expect(apiMocks.putEarliestEpoch).toHaveBeenCalledWith({
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.1:10000",
        earliest_epoch: 3,
      });
    });
  });

  it("shows epoch name in earliest epoch dropdown options", async () => {
    render(Page);

    const earliest = await screen.findByTestId(
      "earliest-epoch-fwd-1/10.0.0.1:10000",
    );

    await waitFor(() => {
      expect(earliest).toHaveTextContent("5 (Main)");
    });
  });

  it("shows local port when stream has one", async () => {
    apiMocks.getStreams.mockResolvedValueOnce({
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 41111,
          stream_epoch: 5,
          paused: false,
        },
      ],
      degraded: false,
      upstream_error: null,
    });
    render(Page);

    expect(await screen.findByText("local port: 41111")).toBeInTheDocument();
  });

  it("refetches epoch options when stream epoch changes in snapshot", async () => {
    render(Page);
    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();

    await waitFor(() => {
      expect(apiMocks.getReplayTargetEpochs).toHaveBeenCalledTimes(1);
    });

    callbacks.onStreamsSnapshot({
      streams: [
        {
          ...fixtures.activeStreamsResponse.streams[0],
          stream_epoch: 6,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    await waitFor(() => {
      expect(apiMocks.getReplayTargetEpochs).toHaveBeenCalledTimes(2);
    });
  });

  it("refetches epoch options when current epoch name changes in snapshot", async () => {
    render(Page);
    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();

    await waitFor(() => {
      expect(apiMocks.getReplayTargetEpochs).toHaveBeenCalledTimes(1);
    });

    callbacks.onStreamsSnapshot({
      streams: [
        {
          ...fixtures.activeStreamsResponse.streams[0],
          current_epoch_name: "Renamed Epoch",
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    await waitFor(() => {
      expect(apiMocks.getReplayTargetEpochs).toHaveBeenCalledTimes(2);
    });
  });

  it("refetches epoch options when a streams snapshot arrives, even if metadata is unchanged", async () => {
    render(Page);
    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();

    await waitFor(() => {
      expect(apiMocks.getReplayTargetEpochs).toHaveBeenCalledTimes(1);
    });

    callbacks.onStreamsSnapshot({
      ...fixtures.activeStreamsResponse,
    });

    await waitFor(() => {
      expect(apiMocks.getReplayTargetEpochs).toHaveBeenCalledTimes(2);
    });
  });

  it("disables earliest dropdown in race mode", async () => {
    apiMocks.getMode.mockResolvedValueOnce({ mode: "race", race_id: "race-1" });
    render(Page);

    const earliest = await screen.findByTestId(
      "earliest-epoch-fwd-1/10.0.0.1:10000",
    );

    expect(earliest).toBeDisabled();
  });

  it("replays a single stream in targeted replay mode", async () => {
    render(Page);

    const modeSelect = await screen.findByTestId("mode-select");
    await fireEvent.change(modeSelect, {
      target: { value: "targeted_replay" },
    });

    const epochInput = await screen.findByTestId(
      "targeted-epoch-fwd-1/10.0.0.1:10000",
    );
    await fireEvent.input(epochInput, { target: { value: "7" } });

    const replayButton = await screen.findByTestId(
      "replay-stream-fwd-1/10.0.0.1:10000",
    );
    await fireEvent.click(replayButton);

    await waitFor(() => {
      expect(apiMocks.putMode).toHaveBeenCalledWith({
        mode: "targeted_replay",
        targets: [
          {
            forwarder_id: "fwd-1",
            reader_ip: "10.0.0.1:10000",
            stream_epoch: 7,
          },
        ],
      });
    });
  });

  it("replays all streams with valid target epochs", async () => {
    apiMocks.getStreams.mockResolvedValueOnce({
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: false,
          local_port: null,
          paused: false,
        },
        {
          forwarder_id: "fwd-2",
          reader_ip: "10.0.0.2:10000",
          subscribed: false,
          local_port: null,
          paused: false,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    render(Page);

    const modeSelect = await screen.findByTestId("mode-select");
    await fireEvent.change(modeSelect, {
      target: { value: "targeted_replay" },
    });

    const epoch1 = await screen.findByTestId(
      "targeted-epoch-fwd-1/10.0.0.1:10000",
    );
    const epoch2 = await screen.findByTestId(
      "targeted-epoch-fwd-2/10.0.0.2:10000",
    );
    await fireEvent.input(epoch1, { target: { value: "3" } });
    await fireEvent.input(epoch2, { target: { value: "9" } });

    const replayAll = await screen.findByTestId("replay-all-btn");
    await fireEvent.click(replayAll);

    await waitFor(() => {
      expect(apiMocks.putMode).toHaveBeenCalledWith({
        mode: "targeted_replay",
        targets: [
          {
            forwarder_id: "fwd-1",
            reader_ip: "10.0.0.1:10000",
            stream_epoch: 3,
          },
          {
            forwarder_id: "fwd-2",
            reader_ip: "10.0.0.2:10000",
            stream_epoch: 9,
          },
        ],
      });
    });
  });
});
