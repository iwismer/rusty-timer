import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import Page from "../routes/admin/+page.svelte";

const apiMocks = vi.hoisted(() => ({
  getStreams: vi.fn().mockResolvedValue({
    streams: [
      {
        forwarder_id: "f1",
        reader_ip: "10.0.0.1:10000",
        subscribed: true,
        local_port: 10100,
        display_alias: "Finish",
      },
      {
        forwarder_id: "f2",
        reader_ip: "10.0.0.2:10000",
        subscribed: true,
        local_port: 10101,
        display_alias: "Start",
      },
    ],
    degraded: false,
    upstream_error: null,
  }),
  getSubscriptions: vi.fn().mockResolvedValue({ subscriptions: [] }),
  resetStreamCursor: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("$lib/api", () => apiMocks);

describe("receiver admin page", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("loads streams and renders cursor reset actions", async () => {
    render(Page);

    expect(
      await screen.findByRole("button", { name: "Reset cursor for Finish" }),
    ).toBeInTheDocument();
  });

  it("calls reset api and shows success feedback", async () => {
    render(Page);

    const resetButton = await screen.findByRole("button", {
      name: "Reset cursor for Finish",
    });
    await fireEvent.click(resetButton);

    await waitFor(() => {
      expect(apiMocks.resetStreamCursor).toHaveBeenCalledWith({
        forwarder_id: "f1",
        reader_ip: "10.0.0.1:10000",
      });
    });

    expect(
      await screen.findByText("Cursor reset for Finish."),
    ).toBeInTheDocument();
  });

  it("shows error feedback when reset fails", async () => {
    apiMocks.resetStreamCursor.mockRejectedValueOnce(new Error("boom"));
    render(Page);

    const resetButton = await screen.findByRole("button", {
      name: "Reset cursor for Finish",
    });
    await fireEvent.click(resetButton);

    expect(
      await screen.findByText("Failed to reset cursor for Finish."),
    ).toBeInTheDocument();
  });

  it("shows display_alias prominently when available", async () => {
    render(Page);

    // Wait for streams to load by checking for a button
    await screen.findByRole("button", { name: "Reset cursor for Finish" });

    // The alias appears in multiple cards; verify at least one is a styled span
    const aliases = screen.getAllByText("Finish");
    expect(aliases.length).toBeGreaterThanOrEqual(1);
    expect(aliases[0].tagName).toBe("SPAN");
    // The alias should be accompanied by the forwarder/reader detail below it
    expect(
      screen.getAllByText("f1 / 10.0.0.1:10000").length,
    ).toBeGreaterThanOrEqual(1);
  });

  it("falls back to forwarder_id / reader_ip when no display_alias", async () => {
    apiMocks.getStreams.mockResolvedValueOnce({
      streams: [
        {
          forwarder_id: "f3",
          reader_ip: "10.0.0.3:10000",
          subscribed: true,
          local_port: 10102,
        },
      ],
      degraded: false,
      upstream_error: null,
    });

    render(Page);

    expect(
      await screen.findByRole("button", {
        name: "Reset cursor for f3 / 10.0.0.3:10000",
      }),
    ).toBeInTheDocument();
    // The fallback label appears in multiple cards
    expect(
      screen.getAllByText("f3 / 10.0.0.3:10000").length,
    ).toBeGreaterThanOrEqual(1);
  });

  it("only disables the active row while reset is in flight", async () => {
    let resolveFirstReset: (() => void) | undefined;
    apiMocks.resetStreamCursor
      .mockImplementationOnce(
        () =>
          new Promise<void>((resolve) => {
            resolveFirstReset = resolve;
          }),
      )
      .mockResolvedValueOnce(undefined);

    render(Page);

    const finishButton = await screen.findByRole("button", {
      name: "Reset cursor for Finish",
    });
    const startButton = await screen.findByRole("button", {
      name: "Reset cursor for Start",
    });

    await fireEvent.click(finishButton);
    expect(finishButton).toBeDisabled();
    expect(startButton).not.toBeDisabled();

    await fireEvent.click(startButton);
    await waitFor(() => {
      expect(apiMocks.resetStreamCursor).toHaveBeenNthCalledWith(2, {
        forwarder_id: "f2",
        reader_ip: "10.0.0.2:10000",
      });
    });

    expect(resolveFirstReset).toBeDefined();
    resolveFirstReset!();
    await waitFor(() => {
      expect(finishButton).not.toBeDisabled();
    });
  });
});
