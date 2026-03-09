import { describe, expect, it } from "vitest";
import { getSection, getField, searchHelp } from "./index";

describe("getSection", () => {
  it("returns the server section for forwarder context", () => {
    const section = getSection("forwarder", "server");
    expect(section).toBeDefined();
    expect(section!.title).toBe("Server Connection");
  });

  it("returns undefined for a nonexistent section", () => {
    expect(getSection("forwarder", "nonexistent")).toBeUndefined();
  });
});

describe("getField", () => {
  it("returns the base_url field from forwarder server section", () => {
    const field = getField("forwarder", "server", "base_url");
    expect(field).toBeDefined();
    expect(field!.label).toBe("Base URL");
  });

  it("returns undefined for a nonexistent field", () => {
    expect(getField("forwarder", "server", "nonexistent")).toBeUndefined();
  });

  it("returns undefined for a nonexistent section", () => {
    expect(getField("forwarder", "nonexistent", "base_url")).toBeUndefined();
  });
});

describe("searchHelp", () => {
  it("returns empty array for empty query", () => {
    expect(searchHelp("")).toEqual([]);
  });

  it("returns empty array for whitespace-only query", () => {
    expect(searchHelp("   ")).toEqual([]);
  });

  it("returns empty array when nothing matches", () => {
    expect(searchHelp("zzz-no-match-xyz")).toEqual([]);
  });

  it("finds forwarder server section when searching for base_url content", () => {
    const results = searchHelp("Base URL");
    expect(results.length).toBeGreaterThan(0);
    const match = results.find(
      (r) => r.context === "forwarder" && r.sectionKey === "server",
    );
    expect(match).toBeDefined();
    expect(match!.matchedFields.some((f) => f.fieldKey === "base_url")).toBe(true);
  });

  it("matches section title", () => {
    const results = searchHelp("Server Connection");
    const match = results.find(
      (r) => r.context === "forwarder" && r.sectionKey === "server",
    );
    expect(match).toBeDefined();
  });

  it("matches case-insensitively", () => {
    const results = searchHelp("BASE URL");
    expect(results.length).toBeGreaterThan(0);
  });

  it("matches tips", () => {
    const results = searchHelp("descriptive name");
    expect(results.length).toBeGreaterThan(0);
    const match = results.find(
      (r) => r.context === "forwarder" && r.sectionKey === "general",
    );
    expect(match).toBeDefined();
    expect(match!.matchedTips.length).toBeGreaterThan(0);
  });
});
