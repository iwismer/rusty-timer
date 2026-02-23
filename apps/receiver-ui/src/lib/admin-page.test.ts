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
  resetStreamCursor: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("$lib/api", () => apiMocks);

describe("receiver admin page", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("loads streams and renders cursor reset actions", async () => {
    render(Page);

    expect(await screen.findByText("Finish")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Reset cursor for Finish" }),
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
