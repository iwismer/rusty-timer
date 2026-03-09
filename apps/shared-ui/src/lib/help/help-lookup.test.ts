import { describe, expect, it } from "vitest";
import { getSection, getField, searchHelp } from "./index";
import { FORWARDER_HELP } from "./forwarder-help";
import { RECEIVER_HELP } from "./receiver-help";
import { RECEIVER_ADMIN_HELP } from "./receiver-admin-help";
import type { HelpContextName, HelpContext } from "./help-types";

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

  it("returns all fields when only section title matches", () => {
    const results = searchHelp("Server Connection");
    const match = results.find(
      (r) => r.context === "forwarder" && r.sectionKey === "server",
    );
    expect(match).toBeDefined();
    expect(match!.matchedFields.length).toBeGreaterThan(0);
    expect(match!.matchedFields.some((f) => f.fieldKey === "base_url")).toBe(true);
  });

  it("matches section overview text", () => {
    const results = searchHelp("IPICO");
    expect(results.length).toBeGreaterThan(0);
    const match = results.find(
      (r) => r.context === "forwarder" && r.sectionKey === "readers",
    );
    expect(match).toBeDefined();
  });

  it("handles sections with empty fields (tips-only sections)", () => {
    const results = searchHelp("purge");
    const match = results.find(
      (r) => r.context === "receiver-admin" && r.sectionKey === "purge_subscriptions",
    );
    expect(match).toBeDefined();
    expect(match!.matchedTips.length).toBeGreaterThan(0);
  });
});

describe("seeAlso cross-reference validation", () => {
  const contexts: Record<HelpContextName, HelpContext> = {
    forwarder: FORWARDER_HELP,
    receiver: RECEIVER_HELP,
    "receiver-admin": RECEIVER_ADMIN_HELP,
  };

  it("all seeAlso references resolve to existing sections", () => {
    const errors: string[] = [];
    for (const [contextName, context] of Object.entries(contexts)) {
      for (const [sectionKey, section] of Object.entries(context)) {
        for (const link of section.seeAlso ?? []) {
          if (!context[link.sectionKey]) {
            errors.push(
              `${contextName}/${sectionKey} -> seeAlso "${link.sectionKey}" does not exist`,
            );
          }
        }
      }
    }
    expect(errors).toEqual([]);
  });
});
