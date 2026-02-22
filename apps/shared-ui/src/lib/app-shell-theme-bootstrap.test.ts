import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

function assertBootstrapThemeScript(appHtml: string): void {
  const storageRead = appHtml.indexOf(
    'localStorage.getItem("rusty-timer-theme")',
  );
  const headPlaceholder = appHtml.indexOf("%sveltekit.head%");

  expect(storageRead).toBeGreaterThan(-1);
  expect(storageRead).toBeLessThan(headPlaceholder);
  expect(appHtml).toContain('saved === "light" || saved === "dark"');
  expect(appHtml).toContain("document.documentElement.style.colorScheme = saved");
}

describe("app shell theme bootstrap", () => {
  it("applies saved explicit theme before hydration in server-ui", () => {
    const appHtml = readFileSync(
      resolve(import.meta.dirname, "../../../server-ui/src/app.html"),
      "utf8",
    );
    assertBootstrapThemeScript(appHtml);
  });

  it("applies saved explicit theme before hydration in forwarder-ui", () => {
    const appHtml = readFileSync(
      resolve(import.meta.dirname, "../../../forwarder-ui/src/app.html"),
      "utf8",
    );
    assertBootstrapThemeScript(appHtml);
  });

  it("applies saved explicit theme before hydration in receiver-ui", () => {
    const appHtml = readFileSync(
      resolve(import.meta.dirname, "../../../receiver-ui/src/app.html"),
      "utf8",
    );
    assertBootstrapThemeScript(appHtml);
  });
});
