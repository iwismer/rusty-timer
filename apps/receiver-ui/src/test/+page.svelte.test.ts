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

  it("auto-applies race id when committed on blur", async () => {
    render(Page);

    const modeSelect = await screen.findByTestId("selection-mode-select");
    await fireEvent.change(modeSelect, { target: { value: "race" } });

    const raceIdInput = await screen.findByTestId("race-id-input");
    await fireEvent.input(raceIdInput, { target: { value: "race-42" } });
    await fireEvent.blur(raceIdInput);

    await waitFor(() => {
      expect(apiMocks.putSelection).toHaveBeenCalledWith(
        expect.objectContaining({
          selection: expect.objectContaining({
            mode: "race",
            race_id: "race-42",
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
  });
});
