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
      "SBC Setup",
      "Admin",
    ]);
    expect(links.find((link) => link.label === "Announcer")?.active).toBe(true);
  });

  it("marks SBC Setup as active on /sbc-setup", () => {
    const links = getLayoutNavLinks("/sbc-setup");
    const sbcLink = links.find((l) => l.label === "SBC Setup");
    expect(sbcLink).toBeDefined();
    expect(sbcLink!.active).toBe(true);
    expect(sbcLink!.href).toBe("/sbc-setup");
  });

  it("marks SBC Setup as inactive on other pages", () => {
    const links = getLayoutNavLinks("/admin");
    const sbcLink = links.find((l) => l.label === "SBC Setup");
    expect(sbcLink).toBeDefined();
    expect(sbcLink!.active).toBe(false);
  });
});
