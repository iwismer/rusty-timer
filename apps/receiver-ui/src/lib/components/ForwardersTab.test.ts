import { render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMocks = vi.hoisted(() => ({
  getForwarders: vi.fn(),
}));

vi.mock("$lib/api", () => apiMocks);

vi.mock("@rusty-timer/shared-ui", () => ({
  ForwarderConfig: () => null,
}));

import ForwardersTab from "./ForwardersTab.svelte";
import { store } from "$lib/store.svelte";

describe("ForwardersTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    store.forwarders = null;
    store.selectedForwarderId = null;
    store.activeTab = "forwarders";
  });

  it("shows an error state when forwarders fail to load", async () => {
    apiMocks.getForwarders.mockRejectedValueOnce(new Error("server offline"));

    render(ForwardersTab);

    await waitFor(() => {
      expect(
        screen.getByText(/Unable to load forwarders:/),
      ).toBeInTheDocument();
    });
    expect(screen.queryByText("Loading forwarders...")).not.toBeInTheDocument();
  });
});
