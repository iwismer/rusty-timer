import { describe, expect, it } from "vitest";
import { shouldBootstrapDashboard } from "./layout-bootstrap";

describe("shouldBootstrapDashboard", () => {
  it("returns false for public announcer routes", () => {
    expect(shouldBootstrapDashboard("/announcer")).toBe(false);
    expect(shouldBootstrapDashboard("/announcer/live")).toBe(false);
  });

  it("returns true for dashboard routes", () => {
    expect(shouldBootstrapDashboard("/")).toBe(true);
    expect(shouldBootstrapDashboard("/announcer-config")).toBe(true);
  });
});
