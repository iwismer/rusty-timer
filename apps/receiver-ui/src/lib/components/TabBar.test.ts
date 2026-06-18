import { render, screen } from "@testing-library/svelte";
import { describe, expect, it } from "vitest";

import TabBar from "./TabBar.svelte";

describe("TabBar", () => {
  it("omits removed dashboard tabs after P2P cutover", () => {
    render(TabBar);

    expect(screen.queryByRole("tab", { name: "Forwarders" })).toBeNull();
    expect(screen.queryByRole("tab", { name: "Races" })).toBeNull();
    expect(screen.queryByRole("tab", { name: "Announcer" })).toBeNull();
  });
});
