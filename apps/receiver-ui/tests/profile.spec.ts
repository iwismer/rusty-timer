import { test, expect } from "@playwright/test";

// These E2E tests run against mocked control API routes.

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
  await page.route("**/api/v1/events", async (route) => {
    await route.fulfill({
      status: 200,
      headers: { "content-type": "text/event-stream" },
      body: "\n",
    });
  });

  await page.route("**/api/v1/status", async (route) => {
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

  await page.route("**/api/v1/profile", async (route) => {
    if (route.request().method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          server_url: "wss://example.com/ws/v1/receivers",
          token: "token",
        }),
      });
      return;
    }

    await route.fulfill({ status: 204, body: "" });
  });

  await page.route("**/api/v1/logs", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ entries: [] }),
    });
  });

  await page.route("**/api/v1/streams", async (route) => {
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
    await mockReceiverApi(page, []);
  });

  test("renders status section", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator('[data-testid="status-section"]')).toBeVisible();
  });

  test("renders profile section", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator('[data-testid="config-section"]')).toBeVisible();
  });

  test("can fill in and save profile form", async ({ page }) => {
    await page.goto("/");
    await page
      .locator('[data-testid="server-url-input"]')
      .fill("wss://test.example.com");
    await page.locator('[data-testid="token-input"]').fill("test-token");
    // The save button should be present and enabled
    const saveBtn = page.locator('[data-testid="save-config-btn"]');
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
      streamsSection.getByRole("button", { name: /^Subscribe$/ }),
    ).toHaveCount(1);
    await expect(
      streamsSection.getByRole("button", { name: /^Unsubscribe$/ }),
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

    await page.route("**/api/v1/subscriptions", async (route) => {
      await new Promise<void>((resolve) => {
        releasePut = resolve;
      });
      await route.fulfill({ status: 204, body: "" });
    });

    await page.goto("/");
    const first = page.locator('[data-testid="sub-f1/10.0.0.1"]');
    const second = page.locator('[data-testid="sub-f2/10.0.0.2"]');

    await first.click();
    await expect(first).toBeDisabled();
    await expect(second).toBeDisabled();

    releasePut();
  });

  test("subscribe with port override sends numeric payload", async ({
    page,
  }) => {
    let putPayload: unknown = null;

    await mockReceiverApi(page, [
      {
        forwarder_id: "f1",
        reader_ip: "10.0.0.1",
        subscribed: false,
        local_port: null,
        online: true,
      },
    ]);

    await page.route("**/api/v1/subscriptions", async (route) => {
      if (route.request().method() === "PUT") {
        putPayload = route.request().postDataJSON();
        await route.fulfill({ status: 204, body: "" });
        return;
      }

      await route.fulfill({ status: 405, body: "" });
    });

    await page.goto("/");
    await page.locator('[data-testid="port-f1/10.0.0.1"]').fill("9002");
    await page.locator('[data-testid="sub-f1/10.0.0.1"]').click();

    await expect
      .poll(() => putPayload)
      .toEqual({
        subscriptions: [
          {
            forwarder_id: "f1",
            reader_ip: "10.0.0.1",
            local_port_override: 9002,
          },
        ],
      });
  });

  test("clears subscription validation error after successful retry", async ({
    page,
  }) => {
    let putCalls = 0;

    await mockReceiverApi(page, [
      {
        forwarder_id: "f1",
        reader_ip: "10.0.0.1",
        subscribed: false,
        local_port: null,
        online: true,
      },
    ]);

    await page.route("**/api/v1/subscriptions", async (route) => {
      if (route.request().method() === "PUT") {
        putCalls += 1;
        await route.fulfill({ status: 204, body: "" });
        return;
      }

      await route.fulfill({ status: 405, body: "" });
    });

    await page.goto("/");

    await page.locator('[data-testid="port-f1/10.0.0.1"]').fill("70000");
    await page.locator('[data-testid="sub-f1/10.0.0.1"]').click();
    await expect(
      page.getByText("Port override must be in range 1-65535."),
    ).toBeVisible();

    await page.locator('[data-testid="port-f1/10.0.0.1"]').fill("9002");
    await page.locator('[data-testid="sub-f1/10.0.0.1"]').click();

    await expect.poll(() => putCalls).toBe(1);
    await expect(
      page.getByText("Port override must be in range 1-65535."),
    ).toHaveCount(0);
  });
});
