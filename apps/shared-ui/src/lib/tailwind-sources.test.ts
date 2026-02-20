import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

describe("shared Tailwind source configuration", () => {
  it("includes shared UI component files as scan sources", () => {
    const tokensCssPath = resolve(
      import.meta.dirname,
      "../styles/tokens.css",
    );
    const tokensCss = readFileSync(tokensCssPath, "utf8");

    expect(tokensCss).toContain('@source "../components/**/*.svelte";');
  });
});
