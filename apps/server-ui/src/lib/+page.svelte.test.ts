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
    resetEpoch: vi.fn(),
    getRaceStreamEpochMappings: vi.fn(),
    setStreamEpochRace: vi.fn(),
    activateNextStreamEpochForRace: vi.fn(),
  };
});

import * as api from "$lib/api";
import RootPage from "../routes/+page.svelte";
import StreamDetailPage from "../routes/streams/[streamId]/+page.svelte";

const stream = {
  stream_id: "abc-123",
  forwarder_id: "fwd-1",
  reader_ip: "10.0.0.1:10000",
  display_alias: "Main Stream",
  forwarder_display_name: null,
  online: true,
  stream_epoch: 1,
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
    event_count: 6,
    first_event_at: "2026-02-22T12:00:00Z",
    last_event_at: "2026-02-22T12:30:00Z",
    name: "Heat 1",
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

describe("root streams page", () => {
  it("renders the streams heading", () => {
    render(RootPage);

    expect(screen.getByTestId("streams-heading")).toHaveTextContent("Streams");
  });
});

describe("stream detail page activate-next", () => {
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
    vi.mocked(api.getRaceStreamEpochMappings).mockImplementation(
      async (raceId: string) => {
        if (raceId === "race-1") {
          return {
            mappings: [
              {
                stream_id: "abc-123",
                forwarder_id: "fwd-1",
                reader_ip: "10.0.0.1:10000",
                stream_epoch: 1,
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
    vi.mocked(api.resetEpoch).mockResolvedValue();
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

  it("keeps shared advance action enabled for dirty rows and uses saved mapping", async () => {
    render(StreamDetailPage);

    const select = await screen.findByTestId("epoch-race-select-1");
    const activateNext = screen.getByTestId("epoch-race-advance-next-btn");
    const saveButton = screen.getByTestId("epoch-race-save-1");

    expect(activateNext).not.toBeDisabled();
    expect(saveButton).toBeDisabled();

    await fireEvent.change(select, { target: { value: "race-2" } });
    expect(saveButton).not.toBeDisabled();
    expect(activateNext).not.toBeDisabled();

    await fireEvent.click(activateNext);
    expect(api.activateNextStreamEpochForRace).toHaveBeenCalledWith(
      "race-1",
      "abc-123",
    );
  });

  it("calls resetEpoch when advancing an unmapped current epoch", async () => {
    vi.mocked(api.getRaceStreamEpochMappings).mockResolvedValue({
      mappings: [],
    });

    render(StreamDetailPage);

    const activateNext = await screen.findByTestId(
      "epoch-race-advance-next-btn",
    );
    await waitFor(() => {
      expect(activateNext).not.toBeDisabled();
    });

    await fireEvent.click(activateNext);
    expect(api.resetEpoch).toHaveBeenCalledWith("abc-123");
    expect(api.activateNextStreamEpochForRace).not.toHaveBeenCalled();
  });

  it("enables shared advance action when stream epoch matches and is_current is false", async () => {
    vi.mocked(api.getStreamEpochs).mockResolvedValue([
      {
        epoch: 1,
        event_count: 6,
        first_event_at: "2026-02-22T12:00:00Z",
        last_event_at: "2026-02-22T12:30:00Z",
        name: "Heat 1",
        is_current: false,
      },
    ]);

    render(StreamDetailPage);

    const activateNext = await screen.findByTestId(
      "epoch-race-advance-next-btn",
    );
    await waitFor(() => {
      expect(activateNext).not.toBeDisabled();
    });

    await fireEvent.click(activateNext);
    expect(api.activateNextStreamEpochForRace).toHaveBeenCalledWith(
      "race-1",
      "abc-123",
    );
  });

  it("shows pending state while shared advance action is in flight", async () => {
    const pending = deferred<void>();
    vi.mocked(api.activateNextStreamEpochForRace).mockReturnValueOnce(
      pending.promise,
    );

    render(StreamDetailPage);

    const activateNext = await screen.findByTestId(
      "epoch-race-advance-next-btn",
    );
    await fireEvent.click(activateNext);

    expect(activateNext).toBeDisabled();
    expect(activateNext).toHaveTextContent("Advancing...");

    pending.resolve();

    // After the API resolves, button transitions to "Reloading..."
    // while waiting for the SSE-driven table refresh
    await waitFor(() => {
      expect(activateNext).toHaveTextContent("Reloading...");
    });
    expect(activateNext).toBeDisabled();
    expect(api.activateNextStreamEpochForRace).toHaveBeenCalledWith(
      "race-1",
      "abc-123",
    );
  });

  it("shows error state when shared advance action fails", async () => {
    vi.mocked(api.activateNextStreamEpochForRace).mockRejectedValueOnce(
      new Error("boom"),
    );

    render(StreamDetailPage);

    const activateNext = await screen.findByTestId(
      "epoch-race-advance-next-btn",
    );
    await fireEvent.click(activateNext);

    await waitFor(() => {
      expect(activateNext).toHaveTextContent("Advance failed");
    });
  });
});
