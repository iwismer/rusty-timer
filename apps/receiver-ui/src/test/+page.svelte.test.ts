import { render, screen } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";

import Page from "../routes/+page.svelte";

vi.mock("$lib/api", () => ({
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
  putProfile: vi.fn().mockResolvedValue(undefined),
  checkForUpdate: vi.fn().mockResolvedValue({ status: "up_to_date" }),
  downloadUpdate: vi.fn().mockResolvedValue({ status: "failed" }),
  connect: vi.fn().mockResolvedValue(undefined),
  disconnect: vi.fn().mockResolvedValue(undefined),
  applyUpdate: vi.fn().mockResolvedValue(undefined),
  putSubscriptions: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("$lib/sse", () => ({
  initSSE: vi.fn(),
  destroySSE: vi.fn(),
}));

describe("receiver page", () => {
  it("renders receiver heading", async () => {
    render(Page);

    expect(
      screen.getByRole("heading", { name: "Receiver" }),
    ).toBeInTheDocument();
  });
});
