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
              announcement_id: 1,
              bib: 111,
              display_name: "Runner One",
              reader_timestamp: "10:00:00",
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
    expect(screen.queryByText(/chip\s+000000111111/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/reader\s+10:00:00/i)).not.toBeInTheDocument();
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
        announcement_id: 1,
        bib: 222,
        display_name: "Runner Two",
        reader_timestamp: "10:00:01",
      },
    });

    const row = await screen.findByTestId("announcer-row-1");
    await waitFor(() => {
      expect(row.className).toContain("flash-new");
    });
  });

  it("does not let stale snapshot overwrite a newer SSE update", async () => {
    const AnnouncerPage = (await import("../routes/announcer/+page.svelte"))
      .default;
    let resolveFetch: (value: unknown) => void = () => undefined;
    mockFetch.mockReturnValue(
      new Promise((resolve) => {
        resolveFetch = resolve;
      }),
    );

    render(AnnouncerPage);
    await waitFor(() => {
      expect(MockEventSource.instances).toHaveLength(1);
    });

    const es = MockEventSource.instances[0];
    es.emit("announcer_update", {
      finisher_count: 1,
      row: {
        announcement_id: 1,
        bib: 222,
        display_name: "Runner Two",
        reader_timestamp: "10:00:01",
      },
    });

    resolveFetch(makeResponse(200, makeState({ finisher_count: 0, rows: [] })));

    expect(await screen.findByText("Runner Two")).toBeInTheDocument();
    expect(screen.getByText(/Finishers announced:\s*1/)).toBeInTheDocument();
  });
});
