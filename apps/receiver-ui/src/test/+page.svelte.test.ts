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

vi.mock("$lib/sse", () => ({
  initSSE: vi.fn(),
  destroySSE: vi.fn(),
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

  it("auto-applies selection mode changes", async () => {
    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    await waitFor(() => {
      expect(apiMocks.putSelection).toHaveBeenCalledWith(
        expect.objectContaining({
          selection: expect.objectContaining({ mode: "race" }),
        }),
      );
    });
  });

  it("uses race dropdown and auto-applies selected race id", async () => {
    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    const raceIdSelect = await screen.findByTestId("race-id-select");
    expect(raceIdSelect.tagName).toBe("SELECT");
    await fireEvent.change(raceIdSelect, { target: { value: "race-1" } });

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

  it("auto-applies epoch scope and replay policy changes", async () => {
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

  it("shows inline row validation errors and does not submit targeted payload when invalid", async () => {
    render(Page);

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "targeted" },
    });

    apiMocks.putSelection.mockClear();
    const streamSelect = await screen.findByTestId("targeted-row-stream-0");
    await fireEvent.change(streamSelect, { target: { value: "" } });

    const epochInput = await screen.findByTestId("targeted-row-epoch-0");
    await fireEvent.input(epochInput, { target: { value: "" } });
    await fireEvent.blur(epochInput);

    expect(await screen.findByTestId("targeted-row-error-0")).toHaveTextContent(
      "Select a stream",
    );
    expect(await screen.findByTestId("targeted-row-error-0")).toHaveTextContent(
      "Stream epoch is required",
    );
    expect(apiMocks.putSelection).not.toHaveBeenCalled();
  });

  it("serializes valid targeted rows into replay_targets payload", async () => {
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

    const epochInput = await screen.findByTestId("targeted-row-epoch-0");
    await fireEvent.input(epochInput, { target: { value: "3" } });
    await fireEvent.blur(epochInput);

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

  it("applies latest selection after in-flight request settles", async () => {
    const firstApply = deferred<void>();
    apiMocks.putSelection.mockReset();
    apiMocks.putSelection
      .mockReturnValueOnce(firstApply.promise)
      .mockResolvedValue(undefined);

    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "live_only" },
    });

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

  it("retries queued latest selection after in-flight request fails", async () => {
    const firstApply = deferred<void>();
    apiMocks.putSelection.mockReset();
    apiMocks.putSelection
      .mockReturnValueOnce(firstApply.promise)
      .mockResolvedValue(undefined);

    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    const replayPolicySelect = await screen.findByTestId(
      "replay-policy-select",
    );
    await fireEvent.change(replayPolicySelect, {
      target: { value: "live_only" },
    });

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
});
