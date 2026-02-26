import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/svelte";

const mockFetch = vi.fn();
vi.stubGlobal("fetch", mockFetch);

type EventHandler = (event: { data: string }) => void;

class MockEventSource {
  static instances: MockEventSource[] = [];
  url: string;
  private handlers: Record<string, EventHandler[]> = {};

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
  }

  addEventListener(event: string, handler: EventHandler): void {
    this.handlers[event] ??= [];
    this.handlers[event].push(handler);
  }

  close(): void {
    // no-op
  }

  emit(event: string, data: unknown): void {
    for (const handler of this.handlers[event] ?? []) {
      handler({ data: JSON.stringify(data) });
    }
  }
}

vi.stubGlobal("EventSource", MockEventSource);

function makeState(overrides?: Record<string, unknown>) {
  return {
    enabled: true,
    enabled_until: "2026-02-27T10:00:00Z",
    selected_stream_ids: ["stream-1"],
    max_list_size: 25,
    updated_at: "2026-02-26T10:00:00Z",
    public_enabled: true,
    finisher_count: 0,
    rows: [],
    ...overrides,
  };
}

function makeResponse(status: number, body: unknown) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => JSON.stringify(body),
  };
}

beforeEach(() => {
  mockFetch.mockReset();
  MockEventSource.instances = [];
});

describe("public announcer page", () => {
  it("renders disabled state when public announcer is not enabled", async () => {
    const AnnouncerPage = (await import("../routes/announcer/+page.svelte"))
      .default;
    mockFetch.mockResolvedValue(
      makeResponse(200, makeState({ public_enabled: false })),
    );

    render(AnnouncerPage);

    expect(
      await screen.findByText(/announcer screen is disabled/i),
    ).toBeInTheDocument();
  });

  it("renders disclaimer and current rows from snapshot", async () => {
    const AnnouncerPage = (await import("../routes/announcer/+page.svelte"))
      .default;
    mockFetch.mockResolvedValue(
      makeResponse(
        200,
        makeState({
          finisher_count: 1,
          rows: [
            {
              stream_id: "stream-1",
              seq: 1,
              chip_id: "000000111111",
              bib: 111,
              display_name: "Runner One",
              reader_timestamp: "10:00:00",
              received_at: "2026-02-26T10:00:00Z",
            },
          ],
        }),
      ),
    );

    render(AnnouncerPage);

    expect(
      await screen.findByText(/not official results/i),
    ).toBeInTheDocument();
    expect(screen.getByText("Runner One")).toBeInTheDocument();
  });

  it("applies flash class when an announcer_update SSE event arrives", async () => {
    const AnnouncerPage = (await import("../routes/announcer/+page.svelte"))
      .default;
    mockFetch.mockResolvedValue(makeResponse(200, makeState()));

    render(AnnouncerPage);
    await screen.findByText("Waiting for first finisher...");
    const es = MockEventSource.instances[0];
    es.emit("announcer_update", {
      finisher_count: 1,
      row: {
        stream_id: "stream-1",
        seq: 2,
        chip_id: "000000222222",
        bib: 222,
        display_name: "Runner Two",
        reader_timestamp: "10:00:01",
        received_at: "2026-02-26T10:00:01Z",
      },
    });

    const row = await screen.findByTestId("announcer-row-000000222222");
    await waitFor(() => {
      expect(row.className).toContain("flash-new");
    });
  });
});
