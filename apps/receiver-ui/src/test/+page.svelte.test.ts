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

  it("applies live mode with selected stream on Apply Mode", async () => {
    render(Page);

    const include = await screen.findByTestId(
      "live-stream-toggle-fwd-1/10.0.0.1:10000",
    );
    await fireEvent.click(include);

    const apply = await screen.findByTestId("save-mode-btn");
    await fireEvent.click(apply);

    await waitFor(() => {
      expect(apiMocks.putMode).toHaveBeenCalledWith({
        mode: "live",
        streams: [{ forwarder_id: "fwd-1", reader_ip: "10.0.0.1:10000" }],
        earliest_epochs: [],
      });
    });
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

    const button = await screen.findByTestId("pause-resume-fwd-1/10.0.0.1:10000");
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

  it("prevents overlapping pause/resume stream actions while one is in flight", async () => {
    const pauseDeferred = deferred<void>();
    apiMocks.pauseStream.mockImplementationOnce(() => pauseDeferred.promise);

    render(Page);

    const button = await screen.findByTestId("pause-resume-fwd-1/10.0.0.1:10000");
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
      expect(resumeAll).not.toBeDisabled();
    });
  });

  it("does not clobber unsaved local mode edits when loadAll hydration resolves", async () => {
    const firstHydrationDeferred = deferred<{
      mode: "race";
      race_id: string;
    }>();
    apiMocks.getMode.mockImplementationOnce(() => firstHydrationDeferred.promise);

    render(Page);

    const modeSelect = await screen.findByTestId("mode-select");

    await fireEvent.change(modeSelect, { target: { value: "targeted_replay" } });

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

  it("releases stream action busy state when pause stream rejects", async () => {
    const rejection = new Error("pause failed");
    apiMocks.pauseStream.mockRejectedValueOnce(rejection);

    render(Page);

    const button = await screen.findByTestId("pause-resume-fwd-1/10.0.0.1:10000");
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
      expect(resumeAll).not.toBeDisabled();
    });
    expect(screen.getByText(String(rejection))).toBeInTheDocument();
  });

  it("disables earliest epoch controls in race mode", async () => {
    apiMocks.getMode.mockResolvedValueOnce({ mode: "race", race_id: "race-1" });
    render(Page);

    const earliest = await screen.findByTestId("earliest-epoch-fwd-1/10.0.0.1:10000");
    const setEarliest = await screen.findByTestId(
      "apply-earliest-fwd-1/10.0.0.1:10000",
    );

    expect(earliest).toBeDisabled();
    expect(setEarliest).toBeDisabled();
  });

  it("replays a single stream in targeted replay mode", async () => {
    render(Page);

    const modeSelect = await screen.findByTestId("mode-select");
    await fireEvent.change(modeSelect, { target: { value: "targeted_replay" } });

    const epochInput = await screen.findByTestId("targeted-epoch-fwd-1/10.0.0.1:10000");
    await fireEvent.input(epochInput, { target: { value: "7" } });

    const replayButton = await screen.findByTestId(
      "replay-stream-fwd-1/10.0.0.1:10000",
    );
    await fireEvent.click(replayButton);

    await waitFor(() => {
      expect(apiMocks.putMode).toHaveBeenCalledWith({
        mode: "targeted_replay",
        targets: [
          { forwarder_id: "fwd-1", reader_ip: "10.0.0.1:10000", stream_epoch: 7 },
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
    await fireEvent.change(modeSelect, { target: { value: "targeted_replay" } });

    const epoch1 = await screen.findByTestId("targeted-epoch-fwd-1/10.0.0.1:10000");
    const epoch2 = await screen.findByTestId("targeted-epoch-fwd-2/10.0.0.2:10000");
    await fireEvent.input(epoch1, { target: { value: "3" } });
    await fireEvent.input(epoch2, { target: { value: "9" } });

    const replayAll = await screen.findByTestId("replay-all-btn");
    await fireEvent.click(replayAll);

    await waitFor(() => {
      expect(apiMocks.putMode).toHaveBeenCalledWith({
        mode: "targeted_replay",
        targets: [
          { forwarder_id: "fwd-1", reader_ip: "10.0.0.1:10000", stream_epoch: 3 },
          { forwarder_id: "fwd-2", reader_ip: "10.0.0.2:10000", stream_epoch: 9 },
        ],
      });
    });
  });
});
