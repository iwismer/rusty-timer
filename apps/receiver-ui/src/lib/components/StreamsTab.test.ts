import { render, screen } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import StreamsTab from "./StreamsTab.svelte";
import { store, streamKey } from "$lib/store.svelte";

describe("StreamsTab", () => {
  beforeEach(() => {
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
