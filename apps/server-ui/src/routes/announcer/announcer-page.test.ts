import { describe, expect, it } from "vitest";
import announcerSource from "./+page.svelte?raw";

describe("server announcer page reset controls", () => {
  it("includes reset button and confirmation flow in template", () => {
    expect(announcerSource).toContain('data-testid="announcer-reset-btn"');
    expect(announcerSource).toContain(
      'data-testid="confirm-announcer-reset-btn"',
    );
    expect(announcerSource).toContain("api.resetAnnouncer()");
  });

  it("resets local keys and status when reset succeeds", () => {
    expect(announcerSource).toContain("seenKeys.clear()");
    expect(announcerSource).toContain("seeded = false");
    expect(announcerSource).toContain("flashKeys = new Set()");
    expect(announcerSource).toContain("finisher_count: 0");
    expect(announcerSource).toContain("announcer_rows: []");
  });

  it("handles remote generation changes in loadStatus", () => {
    expect(announcerSource).toContain(
      "next.announcer_source_generation !== lastGeneration",
    );
  });
});
