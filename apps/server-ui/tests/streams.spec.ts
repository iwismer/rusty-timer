import { test, expect } from "@playwright/test";

// These E2E tests target the SvelteKit dev/preview server.
// The server API is mocked via route interception so no real backend is needed.

const MOCK_STREAMS = [
  {
    stream_id: "stream-uuid-1",
    forwarder_id: "fwd-alpha",
    reader_ip: "192.168.1.100:10000",
    display_alias: "Alpha Reader",
    online: true,
    stream_epoch: 3,
    created_at: "2024-01-01T00:00:00Z",
  },
  {
    stream_id: "stream-uuid-2",
    forwarder_id: "fwd-beta",
    reader_ip: "10.0.0.50:10000",
    display_alias: null,
    online: false,
    stream_epoch: 1,
    created_at: "2024-01-02T00:00:00Z",
  },
];

const MOCK_METRICS = {
  raw_count: 500,
  dedup_count: 480,
  retransmit_count: 20,
  lag: 2300,
  backlog: 3,
};

const MOCK_LIST_METRICS_RESPONSE = {
  raw_count: 500,
  dedup_count: 480,
  retransmit_count: 20,
  lag_ms: 2300,
  epoch_raw_count: 120,
  epoch_dedup_count: 110,
  epoch_retransmit_count: 10,
  epoch_lag_ms: 1500,
  epoch_last_received_at: "2026-01-01T00:00:00Z",
  unique_chips: 75,
};

test.describe("stream list page", () => {
  test.beforeEach(async ({ page }) => {
    await page.route("**/api/v1/events", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: "event: keepalive\ndata: ok\n\n",
      });
    });
    // Intercept the streams API call
    await page.route("**/api/v1/streams", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ streams: MOCK_STREAMS }),
      });
    });
  });

  test("renders page heading", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator('[data-testid="streams-heading"]')).toBeVisible();
  });

  test("renders list of streams", async ({ page }) => {
    await page.goto("/");
    await expect(
      page.locator('[data-testid="stream-list"]').first(),
    ).toBeVisible();
    const items = page.locator('[data-testid="stream-item"]');
    await expect(items).toHaveCount(2);
  });

  test("shows stream display alias when present", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByText("Alpha Reader")).toBeVisible();
  });

  test("shows forwarder_id and reader_ip when no alias", async ({ page }) => {
    await page.goto("/");
    // Second stream has no alias, should show forwarder_id / reader_ip
    await expect(page.getByText(/fwd-beta/)).toBeVisible();
    await expect(
      page.getByRole("link", { name: "10.0.0.50:10000" }),
    ).toBeVisible();
  });

  test("shows online/offline indicator for each stream", async ({ page }) => {
    await page.goto("/");
    const onlineBadge = page.locator('[data-testid="stream-online-badge"]');
    const offlineBadge = page.locator('[data-testid="stream-offline-badge"]');
    await expect(onlineBadge).toHaveCount(1);
    await expect(offlineBadge).toHaveCount(1);
  });

  test("shows link to per-stream detail page", async ({ page }) => {
    await page.goto("/");
    const links = page.locator('[data-testid="stream-detail-link"]');
    await expect(links).toHaveCount(2);
    const firstHref = await links.first().getAttribute("href");
    expect(firstHref).toContain("stream-uuid-1");
  });

  test("configure link targets forwarder config page", async ({ page }) => {
    await page.goto("/");
    const link = page.getByRole("link", { name: "Configure" }).first();
    await expect(link).toHaveAttribute("href", "/forwarders/fwd-alpha/config");
  });

  test("stream list shows epoch for each stream", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByText(/epoch/i).first()).toBeVisible();
  });

  test("retries metrics fetch after transient failure", async ({ page }) => {
    let streamOneMetricsAttempts = 0;
    await page.route("**/api/v1/streams/*/metrics", async (route) => {
      const url = new URL(route.request().url());
      const streamId = url.pathname.split("/")[4];

      if (streamId === "stream-uuid-1") {
        streamOneMetricsAttempts += 1;
        if (streamOneMetricsAttempts === 1) {
          await route.fulfill({
            status: 500,
            contentType: "application/json",
            body: JSON.stringify({
              code: "INTERNAL_ERROR",
              message: "temporary failure",
            }),
          });
          return;
        }
      }

      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(MOCK_LIST_METRICS_RESPONSE),
      });
    });

    await page.goto("/");

    await expect
      .poll(() => streamOneMetricsAttempts, { timeout: 10000 })
      .toBeGreaterThan(1);
    await expect(page.getByText("Reads: 120").first()).toBeVisible();
  });
});

test.describe("per-stream detail page", () => {
  test.beforeEach(async ({ page }) => {
    await page.route("**/api/v1/events", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: "event: keepalive\ndata: ok\n\n",
      });
    });
    await page.route("**/api/v1/streams", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ streams: MOCK_STREAMS }),
      });
    });
    await page.route(
      "**/api/v1/streams/stream-uuid-1/metrics",
      async (route) => {
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(MOCK_METRICS),
        });
      },
    );
    await page.route(
      "**/api/v1/streams/stream-uuid-1/reset-epoch",
      async (route) => {
        await route.fulfill({ status: 204, body: "" });
      },
    );
  });

  test("renders stream detail heading", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    await expect(
      page.locator('[data-testid="stream-detail-heading"]'),
    ).toBeVisible();
  });

  test("shows stream metrics section", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    await expect(page.locator('[data-testid="metrics-section"]')).toBeVisible();
  });

  test("displays raw_count metric", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    await expect(
      page.locator('[data-testid="metric-raw-count"]'),
    ).toBeVisible();
    await expect(
      page.locator('[data-testid="metric-raw-count"]'),
    ).toContainText("500");
  });

  test("displays dedup_count metric", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    await expect(
      page.locator('[data-testid="metric-dedup-count"]'),
    ).toBeVisible();
    await expect(
      page.locator('[data-testid="metric-dedup-count"]'),
    ).toContainText("480");
  });

  test("displays retransmit_count metric", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    await expect(
      page.locator('[data-testid="metric-retransmit-count"]'),
    ).toBeVisible();
    await expect(
      page.locator('[data-testid="metric-retransmit-count"]'),
    ).toContainText("20");
  });

  test("displays lag metric", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    await expect(page.locator('[data-testid="metric-lag"]')).toBeVisible();
  });

  test("displays backlog metric with current default value", async ({
    page,
  }) => {
    await page.goto("/streams/stream-uuid-1");
    await expect(page.locator('[data-testid="metric-backlog"]')).toBeVisible();
    await expect(page.locator('[data-testid="metric-backlog"]')).toContainText(
      "0",
    );
  });

  test("shows export section with raw and csv links", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    await expect(page.locator('[data-testid="export-section"]')).toBeVisible();
  });

  test("export.txt link points to correct API URL", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    const rawLink = page.locator('[data-testid="export-raw-link"]');
    await expect(rawLink).toBeVisible();
    const href = await rawLink.getAttribute("href");
    expect(href).toContain("/api/v1/streams/stream-uuid-1/export.txt");
  });

  test("export.csv link points to correct API URL", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    const csvLink = page.locator('[data-testid="export-csv-link"]');
    await expect(csvLink).toBeVisible();
    const href = await csvLink.getAttribute("href");
    expect(href).toContain("/api/v1/streams/stream-uuid-1/export.csv");
  });

  test("shows back link to stream list", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    const backLink = page.locator('[data-testid="back-link"]');
    await expect(backLink).toBeVisible();
    await backLink.click();
    await expect(page).toHaveURL("/");
  });

  test("reset-epoch button is present", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    await expect(page.locator('[data-testid="reset-epoch-btn"]')).toBeVisible();
  });

  test("reset-epoch button calls API and shows confirmation", async ({
    page,
  }) => {
    await page.goto("/streams/stream-uuid-1");
    const btn = page.locator('[data-testid="reset-epoch-btn"]');
    await btn.click();
    // After reset, confirmation message appears
    await expect(
      page.locator('[data-testid="reset-epoch-result"]'),
    ).toBeVisible();
  });

  test("rename input is present", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    await expect(page.locator('[data-testid="rename-input"]')).toBeVisible();
  });

  test("rename button is present", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    await expect(page.locator('[data-testid="rename-btn"]')).toBeVisible();
  });

  test("can fill in rename input and submit", async ({ page }) => {
    await page.route("**/api/v1/streams/stream-uuid-1", async (route) => {
      if (route.request().method() === "PATCH") {
        const postData = route.request().postDataJSON();
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            ...MOCK_STREAMS[0],
            display_alias: postData.display_alias,
          }),
        });
      } else {
        await route.continue();
      }
    });
    await page.goto("/streams/stream-uuid-1");
    const patchRequest = page.waitForRequest(
      (request) =>
        request.method() === "PATCH" &&
        request.url().endsWith("/api/v1/streams/stream-uuid-1"),
    );
    const input = page.locator('[data-testid="rename-input"]');
    await input.fill("Updated Alpha");
    const btn = page.locator('[data-testid="rename-btn"]');
    await btn.click();
    const request = await patchRequest;
    expect(request.postDataJSON()).toEqual({ display_alias: "Updated Alpha" });
  });
});
