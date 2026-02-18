import { test, expect } from "@playwright/test";

// These E2E tests require the receiver control API to be running at 127.0.0.1:9090.
// In CI they are skipped if the API is not reachable.

type StreamFixture = {
  forwarder_id: string;
  reader_ip: string;
  subscribed: boolean;
  local_port: number | null;
  online?: boolean;
  display_alias?: string;
};

async function mockReceiverApi(
  page: import("@playwright/test").Page,
  streams: StreamFixture[],
): Promise<void> {
  await page.route("http://127.0.0.1:9090/api/v1/events", async (route) => {
    await route.fulfill({
      status: 200,
      headers: { "content-type": "text/event-stream" },
      body: "\n",
    });
  });

  await page.route("http://127.0.0.1:9090/api/v1/status", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        connection_state: "connected",
        local_ok: true,
        streams_count: streams.length,
      }),
    });
  });

  await page.route("http://127.0.0.1:9090/api/v1/profile", async (route) => {
    if (route.request().method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          server_url: "wss://example.com/ws/v1/receivers",
          token: "token",
          log_level: "info",
        }),
      });
      return;
    }

    await route.fulfill({ status: 204, body: "" });
  });

  await page.route("http://127.0.0.1:9090/api/v1/logs", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ entries: [] }),
    });
  });

  await page.route("http://127.0.0.1:9090/api/v1/streams", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        streams,
        degraded: false,
        upstream_error: null,
      }),
    });
  });
}

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

  test("renders deterministic subscribe/unsubscribe controls", async ({
    page,
  }) => {
    await mockReceiverApi(page, [
      {
        forwarder_id: "f1",
        reader_ip: "10.0.0.1",
        subscribed: false,
        local_port: null,
        online: true,
      },
      {
        forwarder_id: "f2",
        reader_ip: "10.0.0.2",
        subscribed: true,
        local_port: 10002,
        online: true,
      },
    ]);

    await page.goto("/");
    const streamsSection = page.locator('[data-testid="streams-section"]');

    await expect(
      streamsSection.locator('button:has-text("Subscribe")'),
    ).toHaveCount(1);
    await expect(
      streamsSection.locator('button:has-text("Unsubscribe")'),
    ).toHaveCount(1);
  });

  test("disables all subscription toggles while putSubscriptions is in-flight", async ({
    page,
  }) => {
    let releasePut: () => void = () => {};

    await mockReceiverApi(page, [
      {
        forwarder_id: "f1",
        reader_ip: "10.0.0.1",
        subscribed: false,
        local_port: null,
        online: true,
      },
      {
        forwarder_id: "f2",
        reader_ip: "10.0.0.2",
        subscribed: false,
        local_port: null,
        online: true,
      },
    ]);

    await page.route(
      "http://127.0.0.1:9090/api/v1/subscriptions",
      async (route) => {
        await new Promise<void>((resolve) => {
          releasePut = resolve;
        });
        await route.fulfill({ status: 204, body: "" });
      },
    );

    await page.goto("/");
    const first = page.locator('[data-testid="sub-f1/10.0.0.1"]');
    const second = page.locator('[data-testid="sub-f2/10.0.0.2"]');

    await first.click();
    await expect(first).toBeDisabled();
    await expect(second).toBeDisabled();

    releasePut();
  });
});
