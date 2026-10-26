import { fireEvent, render, screen } from "@testing-library/svelte";
import { describe, expect, it } from "vitest";

import TabBar from "./TabBar.svelte";
import { store } from "$lib/store.svelte";

describe("TabBar", () => {
  it("omits legacy unused tabs in race day view", () => {
    render(TabBar);

    expect(screen.queryByRole("tab", { name: "Races" })).toBeNull();
    expect(screen.queryByRole("tab", { name: "Mode" })).toBeNull();
    expect(screen.queryByRole("tab", { name: "Dashboard" })).toBeNull();
  });

  it("exposes the consolidated tabs and Race Day Setup button", async () => {
    render(TabBar);

    expect(screen.getByRole("tab", { name: "Streams" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Forwarders" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Config" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Announcer" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Logs" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Admin" })).toBeInTheDocument();

    await fireEvent.click(screen.getByRole("tab", { name: "Forwarders" }));
    expect(store.activeTab).toBe("connections");

    const setupBtn = screen.getByTestId("race-day-setup-btn");
    expect(setupBtn).toBeInTheDocument();
    expect(setupBtn).toHaveTextContent("Race Day Setup");

    await fireEvent.click(setupBtn);
    expect(store.raceDayModalOpen).toBe(true);
    expect(store.raceDayModalMode).toBe("wizard");
  });
});
