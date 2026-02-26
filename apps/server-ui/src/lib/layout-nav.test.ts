import { describe, expect, it } from "vitest";
import { getLayoutNavLinks } from "./layout-nav";

describe("getLayoutNavLinks", () => {
  it("returns no tabs for public announcer routes", () => {
    expect(getLayoutNavLinks("/announcer")).toEqual([]);
    expect(getLayoutNavLinks("/announcer/live")).toEqual([]);
  });

  it("keeps dashboard tabs for announcer config", () => {
    const links = getLayoutNavLinks("/announcer-config");

    expect(links.map((link) => link.label)).toEqual([
      "Streams",
      "Races",
      "Announcer",
      "Logs",
      "Admin",
    ]);
    expect(links.find((link) => link.label === "Announcer")?.active).toBe(true);
  });
});
