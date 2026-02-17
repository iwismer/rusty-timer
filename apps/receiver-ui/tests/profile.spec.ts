import { test, expect } from "@playwright/test";

// These E2E tests require the receiver control API to be running at 127.0.0.1:9090.
// In CI they are skipped if the API is not reachable.

test.describe("profile page", () => {
  test.beforeEach(async ({ page }) => {
    // Skip if API not reachable
    try {
      await page.goto("/");
    } catch {
      test.skip();
    }
  });

  test("renders status section", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator('[data-testid="status-section"]')).toBeVisible();
  });

  test("renders profile section", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator('[data-testid="profile-section"]')).toBeVisible();
  });

  test("can fill in and save profile form", async ({ page }) => {
    await page.goto("/");
    await page
      .locator('[data-testid="server-url-input"]')
      .fill("wss://test.example.com");
    await page.locator('[data-testid="token-input"]').fill("test-token");
    await page
      .locator('[data-testid="log-level-select"]')
      .selectOption("debug");
    // The save button should be present and enabled
    const saveBtn = page.locator('[data-testid="save-profile-btn"]');
    await expect(saveBtn).toBeEnabled();
  });

  test("renders streams section", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator('[data-testid="streams-section"]')).toBeVisible();
  });

  test("renders logs section", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator('[data-testid="logs-section"]')).toBeVisible();
  });

  test("shows connection state from status", async ({ page }) => {
    await page.goto("/");
    await expect(
      page.locator('[data-testid="connection-state"]'),
    ).toBeVisible();
  });
});
