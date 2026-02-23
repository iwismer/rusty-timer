import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import {
  render,
  screen,
  fireEvent,
  waitFor,
  cleanup,
} from "@testing-library/svelte";
import { tick } from "svelte";
import { setRaces, replaceStreams, resetStores, setMetrics } from "$lib/stores";

const sseMock = vi.hoisted(() => ({
  onStreamUpdated: vi.fn(),
  unsubscribe: vi.fn(),
  listener: null as
    | ((update: { stream_id: string; stream_epoch?: number }) => void)
    | null,
}));

vi.mock("$app/stores", async () => {
  const { readable } = await import("svelte/store");
  return {
    page: readable({ params: { streamId: "abc-123" } }),
  };
});

vi.mock("$lib/api", async () => {
  const actual = await vi.importActual<typeof import("$lib/api")>("$lib/api");
  return {
    ...actual,
    getMetrics: vi.fn(),
    getStreamReads: vi.fn(),
    getStreamEpochs: vi.fn(),
    getRaceStreamEpochMappings: vi.fn(),
    setStreamEpochRace: vi.fn(),
    setStreamEpochName: vi.fn(),
    setForwarderRace: vi.fn(),
    activateNextStreamEpochForRace: vi.fn(),
  };
});

vi.mock("$lib/sse", () => ({
  onStreamUpdated: (
    listener: (update: { stream_id: string; stream_epoch?: number }) => void,
  ) => {
    sseMock.listener = listener;
    sseMock.onStreamUpdated(listener);
    return sseMock.unsubscribe;
  },
}));

import * as api from "$lib/api";
import Page from "../routes/streams/[streamId]/+page.svelte";

const stream = {
  stream_id: "abc-123",
  forwarder_id: "fwd-1",
  reader_ip: "10.0.0.1:10000",
  display_alias: "Main Stream",
  forwarder_display_name: null,
  online: true,
  stream_epoch: 2,
  created_at: "2026-02-22T00:00:00Z",
};

const metrics = {
  raw_count: 10,
  dedup_count: 10,
  retransmit_count: 0,
  lag: 500,
  backlog: 0,
  epoch_raw_count: 6,
  epoch_dedup_count: 6,
  epoch_retransmit_count: 0,
  epoch_lag: 500,
  epoch_last_received_at: "2026-02-22T12:00:00Z",
  unique_chips: 3,
  last_tag_id: null,
  last_reader_timestamp: null,
};

const epochs = [
  {
    epoch: 1,
    event_count: 4,
    first_event_at: "2026-02-22T11:00:00Z",
    last_event_at: "2026-02-22T11:30:00Z",
    name: "Warmup",
    is_current: false,
  },
  {
    epoch: 2,
    event_count: 6,
    first_event_at: "2026-02-22T12:00:00Z",
    last_event_at: "2026-02-22T12:30:00Z",
    name: null,
    is_current: true,
  },
];

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("stream detail page epoch race mapping", () => {
  beforeEach(() => {
    vi.useRealTimers();
    resetStores();
    // resetAllMocks clears calls AND resets implementations
    // (needed to clear dangling mockReturnValueOnce chains from previous tests)
    vi.mocked(api.getStreamEpochs).mockReset();
    vi.mocked(api.getRaceStreamEpochMappings).mockReset();
    vi.clearAllMocks();
    sseMock.listener = null;

    // Reset document.hidden to the default (visible) for every test.
    // Polling tests override this explicitly.
    Object.defineProperty(document, "hidden", {
      value: false,
      writable: true,
      configurable: true,
    });

    vi.mocked(api.getMetrics).mockResolvedValue(metrics);
    vi.mocked(api.getStreamReads).mockResolvedValue({
      reads: [],
      total: 0,
      limit: 100,
      offset: 0,
    });
    vi.mocked(api.getStreamEpochs).mockResolvedValue(epochs);
    vi.mocked(api.getRaceStreamEpochMappings).mockImplementation(
      async (raceId: string) => {
        if (raceId === "race-1") {
          return {
            mappings: [
              {
                stream_id: "abc-123",
                forwarder_id: "fwd-1",
                reader_ip: "10.0.0.1:10000",
                stream_epoch: 2,
                race_id: "race-1",
              },
            ],
          };
        }
        return { mappings: [] };
      },
    );
    vi.mocked(api.setStreamEpochRace).mockResolvedValue({
      stream_id: "abc-123",
      stream_epoch: 1,
      race_id: "race-1",
    });
    vi.mocked(api.setStreamEpochName).mockResolvedValue({
      stream_id: "abc-123",
      stream_epoch: 1,
      name: "Warmup",
    });
    vi.mocked(api.activateNextStreamEpochForRace).mockResolvedValue();

    replaceStreams([stream]);
    setRaces([
      {
        race_id: "race-1",
        name: "Race One",
        created_at: "2026-02-20T00:00:00Z",
        participant_count: 0,
        chip_count: 0,
      },
      {
        race_id: "race-2",
        name: "Race Two",
        created_at: "2026-02-20T00:00:00Z",
        participant_count: 0,
        chip_count: 0,
      },
    ]);
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("marks a row dirty and saves only when Save is clicked", async () => {
    render(Page);

    const select = await screen.findByTestId("epoch-race-select-1");
    const saveButton = screen.getByTestId("epoch-race-save-1");

    expect(saveButton).toBeDisabled();

    await fireEvent.change(select, { target: { value: "race-1" } });

    expect(screen.getByTestId("epoch-race-state-1")).toHaveTextContent(
      "Unsaved",
    );
    expect(saveButton).not.toBeDisabled();

    await fireEvent.click(saveButton);

    expect(api.setStreamEpochRace).toHaveBeenCalledWith("abc-123", 1, "race-1");
  });

  it("hydrates epoch rows with existing saved mappings", async () => {
    vi.mocked(api.getRaceStreamEpochMappings).mockImplementation(
      async (raceId: string) => {
        if (raceId === "race-2") {
          return {
            mappings: [
              {
                stream_id: "abc-123",
                forwarder_id: "fwd-1",
                reader_ip: "10.0.0.1:10000",
                stream_epoch: 1,
                race_id: "race-2",
              },
            ],
          };
        }
        return { mappings: [] };
      },
    );

    render(Page);

    const select = await screen.findByTestId("epoch-race-select-1");
    expect(select).toHaveValue("race-2");
    expect(screen.getByTestId("epoch-race-state-1")).toHaveTextContent("Saved");
    expect(screen.getByTestId("epoch-race-save-1")).toBeDisabled();
  });

  it("treats whitespace-padded saved names as clean on initial load", async () => {
    vi.mocked(api.getStreamEpochs).mockResolvedValue([
      {
        epoch: 1,
        event_count: 4,
        first_event_at: "2026-02-22T11:00:00Z",
        last_event_at: "2026-02-22T11:30:00Z",
        name: "  Warmup  ",
        is_current: false,
      },
      {
        epoch: 2,
        event_count: 6,
        first_event_at: "2026-02-22T12:00:00Z",
        last_event_at: "2026-02-22T12:30:00Z",
        name: null,
        is_current: true,
      },
    ]);

    render(Page);

    await screen.findByTestId("epoch-name-input-1");
    expect(screen.getByTestId("epoch-race-state-1")).toHaveTextContent("Saved");
    expect(screen.getByTestId("epoch-race-save-1")).toBeDisabled();
  });

  it("does not show fully saved status when hydration has partial mapping fetch failures", async () => {
    vi.mocked(api.getRaceStreamEpochMappings).mockImplementation(
      async (raceId: string) => {
        if (raceId === "race-1") {
          throw new Error("mapping fetch failed");
        }
        return { mappings: [] };
      },
    );

    render(Page);

    await screen.findByTestId("epoch-race-select-1");
    expect(screen.getByTestId("epoch-race-state-1")).toHaveTextContent(
      "Unverified",
    );
    expect(screen.getByTestId("epoch-race-state-2")).toHaveTextContent(
      "Unverified",
    );
  });

  it("keeps row unverified when selection changes and reverts without saving", async () => {
    vi.mocked(api.getRaceStreamEpochMappings).mockImplementation(
      async (raceId: string) => {
        if (raceId === "race-1") {
          throw new Error("mapping fetch failed");
        }
        return { mappings: [] };
      },
    );

    render(Page);

    const select = await screen.findByTestId("epoch-race-select-1");
    const saveButton = screen.getByTestId("epoch-race-save-1");
    const status = screen.getByTestId("epoch-race-state-1");

    expect(status).toHaveTextContent("Unverified");
    expect(saveButton).toBeDisabled();

    await fireEvent.change(select, { target: { value: "race-2" } });
    expect(status).toHaveTextContent("Unsaved");
    expect(saveButton).not.toBeDisabled();

    await fireEvent.change(select, { target: { value: "" } });
    expect(status).toHaveTextContent("Unverified");
    expect(saveButton).toBeDisabled();
  });

  it("shows pending state while row save is in flight", async () => {
    const pending = deferred<{
      stream_id: string;
      stream_epoch: number;
      race_id: string | null;
    }>();
    vi.mocked(api.setStreamEpochRace).mockReturnValueOnce(pending.promise);

    render(Page);

    const select = await screen.findByTestId("epoch-race-select-1");
    await fireEvent.change(select, { target: { value: "race-1" } });
    const saveButton = screen.getByTestId("epoch-race-save-1");

    await fireEvent.click(saveButton);

    expect(saveButton).toBeDisabled();
    expect(saveButton).toHaveTextContent("Saving...");
    expect(screen.getByTestId("epoch-race-state-1")).toHaveTextContent(
      "Saving...",
    );

    pending.resolve({
      stream_id: "abc-123",
      stream_epoch: 1,
      race_id: "race-1",
    });

    await waitFor(() => {
      expect(screen.getByTestId("epoch-race-state-1")).toHaveTextContent(
        "Saved",
      );
    });
  });

  it("saves normalized epoch name when row Save is clicked", async () => {
    render(Page);

    const nameInput = await screen.findByTestId("epoch-name-input-1");
    await fireEvent.input(nameInput, { target: { value: "  Finals  " } });
    await fireEvent.click(screen.getByTestId("epoch-race-save-1"));

    await waitFor(() => {
      expect(api.setStreamEpochName).toHaveBeenCalledWith(
        "abc-123",
        1,
        "Finals",
      );
    });
  });

  it("does not call race mapping API when only name changed", async () => {
    render(Page);

    const nameInput = await screen.findByTestId("epoch-name-input-1");
    await fireEvent.input(nameInput, { target: { value: "Finals" } });
    await fireEvent.click(screen.getByTestId("epoch-race-save-1"));

    await waitFor(() => {
      expect(api.setStreamEpochName).toHaveBeenCalledWith(
        "abc-123",
        1,
        "Finals",
      );
    });
    expect(api.setStreamEpochRace).not.toHaveBeenCalled();
  });

  it("normalizes blank epoch name to null on Save", async () => {
    render(Page);

    const nameInput = await screen.findByTestId("epoch-name-input-1");
    await fireEvent.input(nameInput, { target: { value: "   " } });
    await fireEvent.click(screen.getByTestId("epoch-race-save-1"));

    await waitFor(() => {
      expect(api.setStreamEpochName).toHaveBeenCalledWith("abc-123", 1, null);
    });
  });

  it("shows success state after row save succeeds", async () => {
    render(Page);

    const select = await screen.findByTestId("epoch-race-select-1");
    await fireEvent.change(select, { target: { value: "race-1" } });
    await fireEvent.click(screen.getByTestId("epoch-race-save-1"));

    await waitFor(() => {
      expect(screen.getByTestId("epoch-race-state-1")).toHaveTextContent(
        "Saved",
      );
    });
    expect(screen.getByTestId("epoch-race-save-1")).toBeDisabled();
  });

  it("shows error state when row save fails", async () => {
    vi.mocked(api.setStreamEpochRace).mockRejectedValueOnce(new Error("boom"));

    render(Page);

    const select = await screen.findByTestId("epoch-race-select-1");
    await fireEvent.change(select, { target: { value: "race-1" } });
    await fireEvent.click(screen.getByTestId("epoch-race-save-1"));

    await waitFor(() => {
      expect(screen.getByTestId("epoch-race-state-1")).toHaveTextContent(
        "Error",
      );
    });
  });

  it("shows Epoch Race Mapping before Reads", async () => {
    render(Page);

    await screen.findByTestId("epoch-race-select-1");
    const epochHeading = screen.getByText("Epoch Race Mapping");
    const readsHeading = screen.getByText("Reads");

    expect(
      epochHeading.compareDocumentPosition(readsHeading) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("does not render the legacy reset epoch action", async () => {
    render(Page);

    await screen.findByTestId("epoch-race-select-1");
    expect(screen.queryByTestId("reset-epoch-btn")).not.toBeInTheDocument();
  });

  it("renders save action as green", async () => {
    render(Page);

    const saveButton = await screen.findByTestId("epoch-race-save-1");
    expect(saveButton.className).toContain("bg-status-ok-bg");
  });

  it("advances to next epoch with the shared action", async () => {
    render(Page);

    const advanceButton = await screen.findByTestId(
      "epoch-race-advance-next-btn",
    );
    await waitFor(() => {
      expect(advanceButton).not.toBeDisabled();
    });
    await fireEvent.click(advanceButton);

    expect(api.activateNextStreamEpochForRace).toHaveBeenCalledWith(
      "race-1",
      "abc-123",
    );
  });

  it("reloads epoch race mappings on matching stream epoch updates", async () => {
    vi.mocked(api.getStreamEpochs)
      .mockResolvedValueOnce(epochs)
      .mockResolvedValueOnce([
        ...epochs,
        {
          epoch: 3,
          event_count: 0,
          first_event_at: null,
          last_event_at: null,
          name: null,
          is_current: true,
        },
      ]);

    render(Page);
    await screen.findByTestId("epoch-race-select-1");
    expect(api.getStreamEpochs).toHaveBeenCalledTimes(1);

    sseMock.listener?.({ stream_id: "abc-123", stream_epoch: 3 });

    await waitFor(() => {
      expect(api.getStreamEpochs).toHaveBeenCalledTimes(2);
    });
    expect(
      await screen.findByTestId("epoch-race-select-3"),
    ).toBeInTheDocument();
  });

  it("ignores same-stream updates without numeric epoch", async () => {
    render(Page);
    await screen.findByTestId("epoch-race-select-1");
    expect(api.getStreamEpochs).toHaveBeenCalledTimes(1);

    sseMock.listener?.({ stream_id: "abc-123" });

    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(api.getStreamEpochs).toHaveBeenCalledTimes(1);
  });

  it("ignores epoch updates for other streams", async () => {
    render(Page);
    await screen.findByTestId("epoch-race-select-1");
    expect(api.getStreamEpochs).toHaveBeenCalledTimes(1);

    sseMock.listener?.({ stream_id: "other-stream", stream_epoch: 3 });

    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(api.getStreamEpochs).toHaveBeenCalledTimes(1);
  });

  it("unsubscribes stream update listener on unmount", () => {
    const { unmount } = render(Page);

    expect(sseMock.onStreamUpdated).toHaveBeenCalledTimes(1);
    unmount();
    expect(sseMock.unsubscribe).toHaveBeenCalledTimes(1);
  });
  it("keeps advance button disabled and shows Reloading until SSE-triggered table reload completes", async () => {
    const epochReload = deferred<typeof epochs>();
    vi.mocked(api.getStreamEpochs)
      .mockResolvedValueOnce(epochs) // initial load
      .mockReturnValueOnce(epochReload.promise); // SSE-triggered reload

    render(Page);

    const advanceButton = await screen.findByTestId(
      "epoch-race-advance-next-btn",
    );
    await waitFor(() => {
      expect(advanceButton).not.toBeDisabled();
    });

    // Click advance -- API resolves immediately (default mock)
    await fireEvent.click(advanceButton);

    // After API returns, button should still be disabled and show "Reloading..."
    await waitFor(() => {
      expect(advanceButton).toHaveTextContent("Reloading...");
    });
    expect(advanceButton).toBeDisabled();

    // Simulate SSE triggering the reload
    sseMock.listener?.({ stream_id: "abc-123", stream_epoch: 3 });

    // Button should still be loading since epochReload hasn't resolved
    expect(advanceButton).toBeDisabled();

    // Now resolve the epoch reload
    epochReload.resolve([
      ...epochs,
      {
        epoch: 3,
        event_count: 0,
        first_event_at: "2026-02-22T13:00:00Z",
        last_event_at: "2026-02-22T13:00:00Z",
        name: null,
        is_current: true,
      },
    ]);

    // Wait for the reloaded table to appear, then re-query the button
    // (Svelte recreates the DOM when epochRaceRowsLoading toggles)
    await waitFor(() => {
      expect(screen.getByTestId("epoch-race-select-3")).toBeInTheDocument();
    });
    const updatedButton = screen.getByTestId("epoch-race-advance-next-btn");
    expect(updatedButton).toHaveTextContent("Advance to Next Epoch");
  });

  it("clears advance pending state on API error without waiting for reload", async () => {
    vi.mocked(api.activateNextStreamEpochForRace).mockRejectedValueOnce(
      new Error("server error"),
    );

    render(Page);

    const advanceButton = await screen.findByTestId(
      "epoch-race-advance-next-btn",
    );
    await waitFor(() => {
      expect(advanceButton).not.toBeDisabled();
    });

    await fireEvent.click(advanceButton);

    await waitFor(() => {
      expect(advanceButton).toHaveTextContent("Advance failed");
    });
    // Button should not be in a reloading state
    expect(advanceButton).not.toBeDisabled();
  });

  it("prevents double-click during reload phase", async () => {
    const epochReload = deferred<typeof epochs>();
    vi.mocked(api.getStreamEpochs)
      .mockResolvedValueOnce(epochs)
      .mockReturnValueOnce(epochReload.promise);

    render(Page);

    const advanceButton = await screen.findByTestId(
      "epoch-race-advance-next-btn",
    );
    await waitFor(() => {
      expect(advanceButton).not.toBeDisabled();
    });

    await fireEvent.click(advanceButton);

    await waitFor(() => {
      expect(advanceButton).toHaveTextContent("Reloading...");
    });

    // Try to click again while reloading -- should be disabled
    expect(advanceButton).toBeDisabled();
    await fireEvent.click(advanceButton);

    // Should only have been called once
    expect(api.activateNextStreamEpochForRace).toHaveBeenCalledTimes(1);
  });

  it("updates current epoch event count from live metrics without polling", async () => {
    render(Page);
    await screen.findByTestId("epoch-race-select-1");

    expect(screen.getByTestId("epoch-event-count-1")).toHaveTextContent("4");
    expect(screen.getByTestId("epoch-event-count-2")).toHaveTextContent("6");

    setMetrics("abc-123", {
      ...metrics,
      epoch_dedup_count: 20,
    });

    await waitFor(() => {
      expect(screen.getByTestId("epoch-event-count-2")).toHaveTextContent("20");
    });

    // Non-current epochs should not be rewritten by current-epoch metrics updates.
    expect(screen.getByTestId("epoch-event-count-1")).toHaveTextContent("4");
  });

  it("does not rewrite previous epoch count when stream epoch advances before table reload", async () => {
    const epochReload = deferred<typeof epochs>();
    vi.mocked(api.getStreamEpochs)
      .mockResolvedValueOnce(epochs)
      .mockReturnValueOnce(epochReload.promise);

    render(Page);
    await screen.findByTestId("epoch-race-select-2");
    expect(screen.getByTestId("epoch-event-count-2")).toHaveTextContent("6");

    replaceStreams([
      {
        ...stream,
        stream_epoch: 3,
      },
    ]);

    setMetrics("abc-123", {
      ...metrics,
      epoch_dedup_count: 99,
    });

    await tick();
    expect(screen.getByTestId("epoch-event-count-2")).toHaveTextContent("6");

    sseMock.listener?.({ stream_id: "abc-123", stream_epoch: 3 });
    epochReload.resolve([
      ...epochs,
      {
        epoch: 3,
        event_count: 0,
        first_event_at: "2026-02-22T13:00:00Z",
        last_event_at: "2026-02-22T13:00:00Z",
        name: null,
        is_current: true,
      },
    ]);

    await waitFor(() => {
      expect(screen.getByTestId("epoch-event-count-3")).toHaveTextContent("99");
    });

    setMetrics("abc-123", {
      ...metrics,
      epoch_dedup_count: 100,
    });

    await waitFor(() => {
      expect(screen.getByTestId("epoch-event-count-3")).toHaveTextContent(
        "100",
      );
    });
  });

  it("does not apply stale metrics count to new current epoch after reload", async () => {
    const epochReload = deferred<typeof epochs>();
    vi.mocked(api.getStreamEpochs)
      .mockResolvedValueOnce(epochs)
      .mockReturnValueOnce(epochReload.promise);

    render(Page);
    await screen.findByTestId("epoch-race-select-2");
    expect(screen.getByTestId("epoch-event-count-2")).toHaveTextContent("6");

    replaceStreams([
      {
        ...stream,
        stream_epoch: 3,
      },
    ]);
    sseMock.listener?.({ stream_id: "abc-123", stream_epoch: 3 });

    epochReload.resolve([
      ...epochs,
      {
        epoch: 3,
        event_count: 0,
        first_event_at: "2026-02-22T13:00:00Z",
        last_event_at: "2026-02-22T13:00:00Z",
        name: null,
        is_current: true,
      },
    ]);

    await waitFor(() => {
      expect(screen.getByTestId("epoch-event-count-3")).toHaveTextContent("0");
    });
  });

  it("renders Export CSV and TXT links for each epoch row", async () => {
    render(Page);

    const csvLink1 = await screen.findByTestId("epoch-export-csv-1");
    const csvLink2 = screen.getByTestId("epoch-export-csv-2");
    const txtLink1 = screen.getByTestId("epoch-export-txt-1");
    const txtLink2 = screen.getByTestId("epoch-export-txt-2");

    expect(csvLink1).toBeInTheDocument();
    expect(csvLink2).toBeInTheDocument();
    expect(txtLink1).toBeInTheDocument();
    expect(txtLink2).toBeInTheDocument();

    expect(csvLink1.tagName).toBe("A");
    expect(csvLink2.tagName).toBe("A");
    expect(txtLink1.tagName).toBe("A");
    expect(txtLink2.tagName).toBe("A");

    expect(csvLink1).toHaveTextContent("CSV");
    expect(csvLink2).toHaveTextContent("CSV");
    expect(txtLink1).toHaveTextContent("TXT");
    expect(txtLink2).toHaveTextContent("TXT");

    // Verify hrefs point to the per-epoch export endpoint
    expect(csvLink1.getAttribute("href")).toContain(
      "/api/v1/streams/abc-123/epochs/1/export.csv",
    );
    expect(csvLink2.getAttribute("href")).toContain(
      "/api/v1/streams/abc-123/epochs/2/export.csv",
    );
    expect(txtLink1.getAttribute("href")).toContain(
      "/api/v1/streams/abc-123/epochs/1/export.txt",
    );
    expect(txtLink2.getAttribute("href")).toContain(
      "/api/v1/streams/abc-123/epochs/2/export.txt",
    );

    // Verify download attribute is present
    expect(csvLink1).toHaveAttribute("download");
    expect(csvLink2).toHaveAttribute("download");
    expect(txtLink1).toHaveAttribute("download");
    expect(txtLink2).toHaveAttribute("download");
  });

  it("does not overwrite dirty current-epoch rows during metrics-driven count updates", async () => {
    render(Page);
    await screen.findByTestId("epoch-race-select-2");

    // Make current epoch row dirty.
    const select = screen.getByTestId("epoch-race-select-2");
    await fireEvent.change(select, { target: { value: "race-2" } });
    expect(screen.getByTestId("epoch-race-state-2")).toHaveTextContent(
      "Unsaved",
    );
    expect(screen.getByTestId("epoch-event-count-2")).toHaveTextContent("6");

    setMetrics("abc-123", {
      ...metrics,
      epoch_dedup_count: 99,
    });

    await tick();
    expect(screen.getByTestId("epoch-event-count-2")).toHaveTextContent("6");
    expect(screen.getByTestId("epoch-race-state-2")).toHaveTextContent(
      "Unsaved",
    );
  });
});
