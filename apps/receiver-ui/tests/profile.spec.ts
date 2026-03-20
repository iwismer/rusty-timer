import { expect, test } from "@playwright/test";

type StreamFixture = {
  forwarder_id: string;
  reader_ip: string;
  subscribed: boolean;
  local_port: number | null;
  online?: boolean;
  stream_epoch?: number;
};

async function mockReceiverApi(
  page: import("@playwright/test").Page,
  streams: StreamFixture[] = [],
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
        connection_state: "disconnected",
        local_ok: true,
        streams_count: streams.length,
        receiver_id: "recv-test",
      }),
    });
  });

  await page.route("**/api/v1/profile", async (route) => {
    if (route.request().method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          server_url: "wss://example.com",
          token: "token",
          receiver_id: "recv-test",
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

  await page.route("**/api/v1/mode", async (route) => {
    if (route.request().method() === "GET") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          mode: "live",
          streams: [],
          earliest_epochs: [],
        }),
      });
      return;
    }

    await route.fulfill({ status: 204, body: "" });
  });

  await page.route("**/api/v1/races", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ races: [] }),
    });
  });

  await page.route("**/api/v1/replay-targets/epochs**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        epochs: [
          {
            stream_epoch: 5,
            name: "Main",
            first_seen_at: null,
            race_names: [],
          },
        ],
      }),
    });
  });
}

test.describe("receiver shell", () => {
  test.beforeEach(async ({ page }) => {
    await mockReceiverApi(page, []);
  });

  test("shows the toolbar connect button and no desktop-only update indicator in browser mode", async ({
    page,
  }) => {
    await page.goto("/");

    await expect(page.locator('[data-testid="connect-toggle-btn"]')).toHaveText(
      "Connect",
    );
    await expect(
      page.locator('[data-testid="update-indicator-btn"]'),
    ).toHaveCount(0);
  });

  test("allows editing and saving config", async ({ page }) => {
    let savedProfile: unknown = null;

    await page.route("**/api/v1/profile", async (route) => {
      if (route.request().method() === "GET") {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            server_url: "wss://example.com",
            token: "token",
            receiver_id: "recv-test",
          }),
        });
        return;
      }

      savedProfile = route.request().postDataJSON();
      await route.fulfill({ status: 204, body: "" });
    });

    await page.goto("/");
    await page.getByRole("button", { name: "Config" }).click();
    await page
      .locator('[data-testid="receiver-id-input"]')
      .fill("recv-updated");
    await page
      .locator('[data-testid="server-url-input"]')
      .fill("wss://test.example.com");
    await page.locator('[data-testid="token-input"]').fill("updated-token");
    await page.locator('[data-testid="save-config-btn"]').click();

    await expect
      .poll(() => savedProfile)
      .toEqual({
        server_url: "wss://test.example.com",
        token: "updated-token",
        receiver_id: "recv-updated",
      });
  });
});
