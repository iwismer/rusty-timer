import { describe, expect, it } from "vitest";
import { mapSaveSectionResult } from "./config-api-adapter";

describe("mapSaveSectionResult", () => {
  it("returns restart_needed=false for failed saves", () => {
    expect(mapSaveSectionResult({ ok: false, error: "boom" })).toEqual({
      ok: false,
      error: "boom",
      restart_needed: false,
    });
  });

  it("returns restart_needed=true for successful saves", () => {
    expect(mapSaveSectionResult({ ok: true })).toEqual({
      ok: true,
      error: undefined,
      restart_needed: true,
    });
  });
});
