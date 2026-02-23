import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import { setRaces, replaceStreams, resetStores } from "$lib/stores";

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
    setForwarderRace: vi.fn(),
  };
});

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
    is_current: false,
  },
  {
    epoch: 2,
    event_count: 6,
    first_event_at: "2026-02-22T12:00:00Z",
    last_event_at: "2026-02-22T12:30:00Z",
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
    resetStores();
    vi.clearAllMocks();

    vi.mocked(api.getMetrics).mockResolvedValue(metrics);
    vi.mocked(api.getStreamReads).mockResolvedValue({
      reads: [],
      total: 0,
      limit: 100,
      offset: 0,
    });
    vi.mocked(api.getStreamEpochs).mockResolvedValue(epochs);
    vi.mocked(api.getRaceStreamEpochMappings).mockResolvedValue({
      mappings: [],
    });
    vi.mocked(api.setStreamEpochRace).mockResolvedValue({
      stream_id: "abc-123",
      stream_epoch: 1,
      race_id: "race-1",
    });

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
});
