import { fireEvent, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import StreamsTab from "./StreamsTab.svelte";
import { store, streamKey } from "$lib/store.svelte";

describe("StreamsTab", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe() {}
        disconnect() {}
        unobserve() {}
      },
    );
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      width: 900,
      height: 0,
      top: 0,
      right: 900,
      bottom: 0,
      left: 0,
      x: 0,
      y: 0,
      toJSON() {
        return {};
      },
    });

    const key = streamKey("fwd-1", "10.0.0.1:10000");
    store.modeDraft = "live";
    store.error = null;
    store.streams = {
      streams: [
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          subscribed: true,
          local_port: 10100,
          display_alias: "Finish",
          reads_total: 15,
        },
      ],
      degraded: false,
      upstream_error: null,
    };
    store.lastReads = new Map([
      [
        key,
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          chip_id: "AA:BB:CC:DD",
          timestamp: "2026-03-20T14:23:05.123Z",
          bib: "42",
          name: "Ada Lovelace",
        },
      ],
    ]);
  });

  it("shows metrics in expanded row when available", async () => {
    const key = streamKey("fwd-1", "10.0.0.1:10000");
    store.streamMetrics = new Map([
      [
        key,
        {
          forwarder_id: "fwd-1",
          reader_ip: "10.0.0.1:10000",
          raw_count: 1500,
          dedup_count: 1200,
          retransmit_count: 300,
          lag: 2500,
          epoch_raw_count: 500,
          epoch_dedup_count: 400,
          epoch_retransmit_count: 100,
          unique_chips: 75,
          epoch_last_received_at: "2026-03-21T12:00:00Z",
          epoch_lag: 1000,
        },
      ],
    ]);

    render(StreamsTab);

    // Click to expand the row
    const row = screen.getByText("Finish").closest("tr")!;
    await fireEvent.click(row);

    // Verify lifetime metrics
    expect(screen.getByText("1,500")).toBeInTheDocument(); // raw count
    expect(screen.getByText("1,200")).toBeInTheDocument(); // dedup count
    expect(screen.getByText("300")).toBeInTheDocument(); // retransmit
    expect(screen.getByText("2.5 s")).toBeInTheDocument(); // lag

    // Verify epoch metrics
    expect(screen.getByText("75")).toBeInTheDocument(); // unique chips

    // Verify help text (title attributes)
    expect(
      screen.getByTitle("Total frames received including retransmits"),
    ).toBeInTheDocument();
    expect(
      screen.getByTitle("Distinct chip IDs detected in the current epoch"),
    ).toBeInTheDocument();
  });

  it("shows 'Metrics unavailable' when no metrics data", async () => {
    store.streamMetrics = new Map();

    render(StreamsTab);

    const row = screen.getByText("Finish").closest("tr")!;
    await fireEvent.click(row);

    expect(screen.getByText("Metrics unavailable")).toBeInTheDocument();
  });

  it("renders last read with time only and left-aligned text", () => {
    render(StreamsTab);

    const lastRead = screen.getByText(/14:23:05\.123/);
    expect(lastRead).toBeInTheDocument();
    expect(lastRead).not.toHaveTextContent("2026-03-20");
    expect(lastRead.closest("td")).toHaveClass("text-left");
    expect(lastRead.closest("td")).toHaveClass("w-full");
    expect(screen.getByRole("table")).not.toHaveClass("table-fixed");
    expect(
      screen.getByRole("columnheader", { name: "Stream" }),
    ).not.toHaveClass("w-[120px]");
    expect(screen.getByRole("columnheader", { name: "Stream" })).toHaveClass(
      "w-px",
      "whitespace-nowrap",
    );
  });
});
