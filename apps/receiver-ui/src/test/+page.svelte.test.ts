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
    receiver_id: "recv-test",
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

  it("renders receiver-id input populated from profile", async () => {
    apiMocks.getProfile.mockResolvedValueOnce({
      server_url: "wss://s.com",
      token: "tok",
      update_mode: "check-and-download",
      receiver_id: "recv-from-profile",
    });

    render(Page);

    const input = await screen.findByTestId("receiver-id-input");
    await waitFor(() => {
      expect((input as HTMLInputElement).value).toBe("recv-from-profile");
    });
  });

  it("includes receiver_id when saving profile", async () => {
    apiMocks.getProfile.mockResolvedValueOnce({
      server_url: "wss://s.com",
      token: "tok",
      update_mode: "check-and-download",
      receiver_id: "recv-original",
    });

    render(Page);

    const input = await screen.findByTestId("receiver-id-input");
    await waitFor(() => {
      expect((input as HTMLInputElement).value).toBe("recv-original");
    });

    await fireEvent.input(input, { target: { value: "recv-updated" } });

    const saveBtn = await screen.findByTestId("save-config-btn");
    await fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(apiMocks.putProfile).toHaveBeenCalledWith(
        expect.objectContaining({ receiver_id: "recv-updated" }),
      );
    });
  });

  it("keeps save enabled when config changes during in-flight save", async () => {
    const putProfileDeferred = deferred<void>();
    apiMocks.getProfile.mockResolvedValueOnce({
      server_url: "wss://s.com",
      token: "tok",
      update_mode: "check-and-download",
      receiver_id: "recv-original",
    });
    apiMocks.putProfile.mockImplementationOnce(
      () => putProfileDeferred.promise,
    );

    render(Page);

    const receiverIdInput = (await screen.findByTestId(
      "receiver-id-input",
    )) as HTMLInputElement;
    await waitFor(() => {
      expect(receiverIdInput.value).toBe("recv-original");
    });

    await fireEvent.input(receiverIdInput, { target: { value: "recv-first" } });

    const saveBtn = await screen.findByTestId("save-config-btn");
    await fireEvent.click(saveBtn);

    await waitFor(() => {
      expect(saveBtn).toBeDisabled();
    });

    await fireEvent.input(receiverIdInput, {
      target: { value: "recv-second" },
    });
    putProfileDeferred.resolve();

    await waitFor(() => {
      expect(apiMocks.putProfile).toHaveBeenCalledWith(
        expect.objectContaining({ receiver_id: "recv-first" }),
      );
      expect(saveBtn).toBeEnabled();
    });
  });

  it("loadAll fetches mode and hydrates race state", async () => {
    apiMocks.getMode.mockResolvedValueOnce({ mode: "race", race_id: "race-1" });

    render(Page);

    const modeSelect = await screen.findByTestId("mode-select");
    expect((modeSelect as HTMLSelectElement).value).toBe("race");

    const raceSelect = await screen.findByTestId("race-id-select");
    expect((raceSelect as HTMLSelectElement).value).toBe("race-1");
  });

  it("keeps selected race when race list refresh adds a new race", async () => {
    apiMocks.getMode
      .mockResolvedValueOnce({ mode: "race", race_id: "race-1" })
      .mockResolvedValueOnce({ mode: "race", race_id: "race-1" });
    apiMocks.getRaces
      .mockResolvedValueOnce({
        races: [
          { race_id: "race-1", name: "Race 1", created_at: "2026-01-01" },
        ],
      })
      .mockResolvedValueOnce({
        races: [
          { race_id: "race-1", name: "Race 1", created_at: "2026-01-01" },
          { race_id: "race-2", name: "Race 2", created_at: "2026-01-02" },
        ],
      });

    render(Page);

    const raceSelect = (await screen.findByTestId(
      "race-id-select",
    )) as HTMLSelectElement;
    expect(raceSelect.value).toBe("race-1");

    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();
    callbacks.onResync();

    await waitFor(() => {
      expect(raceSelect.value).toBe("race-1");
      expect(
        Array.from(raceSelect.options).some(
          (option) => option.value === "race-2",
        ),
      ).toBe(true);
    });
  });

  it("keeps selected race when race list refresh fails during resync", async () => {
    apiMocks.getMode
      .mockResolvedValueOnce({ mode: "race", race_id: "race-1" })
      .mockResolvedValueOnce({ mode: "race", race_id: "race-1" });
    apiMocks.getRaces
      .mockResolvedValueOnce({
        races: [
          { race_id: "race-1", name: "Race 1", created_at: "2026-01-01" },
        ],
      })
      .mockRejectedValueOnce(new Error("races unavailable"));

    render(Page);

    const raceSelect = (await screen.findByTestId(
      "race-id-select",
    )) as HTMLSelectElement;
    expect(raceSelect.value).toBe("race-1");

    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();
    callbacks.onResync();

    await waitFor(() => {
      expect(raceSelect.value).toBe("race-1");
      expect(
        Array.from(raceSelect.options).some(
          (option) => option.value === "race-1",
        ),
      ).toBe(true);
    });
  });

  it("hydrates live mode when earliest epochs are omitted", async () => {
    apiMocks.getMode.mockResolvedValueOnce({
      mode: "live",
      streams: [],
    });

    render(Page);

    const modeSelect = await screen.findByTestId("mode-select");
    expect((modeSelect as HTMLSelectElement).value).toBe("live");

    await waitFor(() => {
      expect(screen.queryByText(/TypeError/i)).not.toBeInTheDocument();
    });
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
        },
        {
          forwarder_id: "fwd-2",
          reader_ip: "10.0.0.2:10000",
          subscribed: false,
          local_port: null,
          stream_epoch: 7,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

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
        earliest_epochs: [
          {
            forwarder_id: "fwd-1",
            reader_ip: "10.0.0.1:10000",
            earliest_epoch: 3,
          },
        ],
      });
    });
  });

  it("keeps Apply Mode disabled on initial live hydration without local edits", async () => {
    apiMocks.getMode.mockResolvedValueOnce({
      mode: "live",
      streams: [],
      earliest_epochs: [],
    });
    apiMocks.getStreams.mockResolvedValueOnce({
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: false,
          local_port: null,
          stream_epoch: 5,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    render(Page);

    const apply = await screen.findByTestId("save-mode-btn");
    await waitFor(() => {
      expect(apply).toBeDisabled();
    });
  });

  it("enables Apply Mode after local mode edits when initial mode load fails", async () => {
    apiMocks.getMode.mockRejectedValueOnce(new Error("mode unavailable"));
    render(Page);

    const modeSelect = await screen.findByTestId("mode-select");
    const apply = await screen.findByTestId("save-mode-btn");

    expect(apply).toBeDisabled();

    await fireEvent.change(modeSelect, {
      target: { value: "targeted_replay" },
    });

    await waitFor(() => {
      expect(apply).not.toBeDisabled();
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

  it("prefetches epoch options for loaded streams", async () => {
    render(Page);

    await waitFor(() => {
      expect(apiMocks.getReplayTargetEpochs).toHaveBeenCalledWith({
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.1:10000",
      });
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

    const epochSelect = await screen.findByTestId(
      "targeted-epoch-fwd-1/10.0.0.1:10000",
    );
    expect(epochSelect.tagName).toBe("SELECT");
    await fireEvent.change(epochSelect, { target: { value: "9" } });

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
            stream_epoch: 9,
          },
        ],
      });
    });
  });

  it("replays a single stream with current epoch when epoch options are unavailable", async () => {
    apiMocks.getReplayTargetEpochs.mockRejectedValueOnce(
      new Error("epochs unavailable"),
    );
    render(Page);

    const modeSelect = await screen.findByTestId("mode-select");
    await fireEvent.change(modeSelect, {
      target: { value: "targeted_replay" },
    });

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
            stream_epoch: 5,
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
    expect(epoch1.tagName).toBe("SELECT");
    expect(epoch2.tagName).toBe("SELECT");
    await fireEvent.change(epoch1, { target: { value: "3" } });
    await fireEvent.change(epoch2, { target: { value: "9" } });

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

  it("falls back to API-returned epoch when hydrated targeted epoch is unavailable", async () => {
    render(Page);

    const callbacks = sseMocks.initSSE.mock.calls[0]?.[0];
    expect(callbacks).toBeTruthy();

    callbacks.onModeChanged({
      mode: "targeted_replay",
      targets: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          stream_epoch: 7,
        },
      ],
    });

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
            stream_epoch: 5,
          },
        ],
      });
    });
  });

  it("shows Connect button when disconnected", async () => {
    render(Page);

    const btn = await screen.findByTestId("connect-toggle-btn");
    expect(btn).toHaveTextContent("Connect");
    expect(btn).not.toBeDisabled();
  });

  it("shows Disconnect button when connected", async () => {
    apiMocks.getStatus.mockResolvedValueOnce({
      connection_state: "connected",
      local_ok: true,
      streams_count: 1,
      receiver_id: "recv-test",
    });

    render(Page);

    const btn = await screen.findByTestId("connect-toggle-btn");
    await waitFor(() => {
      expect(btn).toHaveTextContent("Disconnect");
    });
    expect(btn).not.toBeDisabled();
  });

  it("shows disabled Connecting button in transitional state", async () => {
    apiMocks.getStatus.mockResolvedValueOnce({
      connection_state: "connecting",
      local_ok: true,
      streams_count: 0,
      receiver_id: "recv-test",
    });

    render(Page);

    const btn = await screen.findByTestId("connect-toggle-btn");
    await waitFor(() => {
      expect(btn).toHaveTextContent("Connecting…");
      expect(btn).toBeDisabled();
    });
  });

  it("calls connect when clicking Connect toggle", async () => {
    render(Page);

    const btn = await screen.findByTestId("connect-toggle-btn");
    await fireEvent.click(btn);

    await waitFor(() => {
      expect(apiMocks.connect).toHaveBeenCalledTimes(1);
    });
  });

  it("calls disconnect when clicking Disconnect toggle", async () => {
    apiMocks.getStatus.mockResolvedValueOnce({
      connection_state: "connected",
      local_ok: true,
      streams_count: 1,
      receiver_id: "recv-test",
    });

    render(Page);

    const btn = await screen.findByTestId("connect-toggle-btn");
    await waitFor(() => {
      expect(btn).toHaveTextContent("Disconnect");
    });
    await fireEvent.click(btn);

    await waitFor(() => {
      expect(apiMocks.disconnect).toHaveBeenCalledTimes(1);
    });
  });

  it("shows disabled Disconnecting button in disconnecting state", async () => {
    apiMocks.getStatus.mockResolvedValueOnce({
      connection_state: "disconnecting",
      local_ok: true,
      streams_count: 0,
      receiver_id: "recv-test",
    });

    render(Page);

    const btn = await screen.findByTestId("connect-toggle-btn");
    await waitFor(() => {
      expect(btn).toHaveTextContent("Disconnecting…");
      expect(btn).toBeDisabled();
    });
  });
});
