import { describe, expect, it } from "vitest";
import { filterSectionContent } from "./help-dialog";
import type { SectionHelp } from "./help/help-types";

const testSection: SectionHelp = {
  title: "Test Section",
  overview: "A section for testing.",
  fields: {
    alpha: {
      label: "Alpha Field",
      summary: "Alpha summary about widgets.",
      detailHtml: "Detailed info about alpha.",
    },
    beta: {
      label: "Beta Field",
      summary: "Beta summary about gadgets.",
      detailHtml: "Detailed info about beta.",
      default: "42",
    },
  },
  tips: ["Check the widgets first.", "Gadgets need calibration."],
};

describe("filterSectionContent", () => {
  it("returns all fields and tips for empty query", () => {
    const result = filterSectionContent(testSection, "");
    expect(result.fields).toHaveLength(2);
    expect(result.tips).toHaveLength(2);
  });

  it("returns all fields and tips for whitespace query", () => {
    const result = filterSectionContent(testSection, "   ");
    expect(result.fields).toHaveLength(2);
    expect(result.tips).toHaveLength(2);
  });

  it("filters to matching fields", () => {
    const result = filterSectionContent(testSection, "widgets");
    expect(result.fields).toHaveLength(1);
    expect(result.fields[0].fieldKey).toBe("alpha");
  });

  it("filters tips", () => {
    const result = filterSectionContent(testSection, "calibration");
    expect(result.tips).toHaveLength(1);
    expect(result.tips[0]).toContain("calibration");
  });

  it("matches case-insensitively", () => {
    const result = filterSectionContent(testSection, "WIDGETS");
    expect(result.fields).toHaveLength(1);
    expect(result.fields[0].fieldKey).toBe("alpha");
  });

  it("returns empty arrays when nothing matches", () => {
    const result = filterSectionContent(testSection, "zzz-no-match");
    expect(result.fields).toHaveLength(0);
    expect(result.tips).toHaveLength(0);
  });

  it("matches on default field", () => {
    const result = filterSectionContent(testSection, "42");
    expect(result.fields).toHaveLength(1);
    expect(result.fields[0].fieldKey).toBe("beta");
  });

  it("handles section with no tips", () => {
    const noTips: SectionHelp = { ...testSection, tips: undefined };
    const result = filterSectionContent(noTips, "");
    expect(result.tips).toHaveLength(0);
  });
});
