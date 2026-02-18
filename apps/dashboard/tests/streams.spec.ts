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

test.describe("stream list page", () => {
  test.beforeEach(async ({ page }) => {
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
    await expect(page.locator('[data-testid="stream-list"]')).toBeVisible();
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
    await expect(page.getByText(/10\.0\.0\.50:10000/)).toBeVisible();
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

  test("stream list shows epoch for each stream", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByText(/epoch/i)).toBeVisible();
  });
});

test.describe("stream list rename flow", () => {
  test.beforeEach(async ({ page }) => {
    await page.route("**/api/v1/streams", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ streams: MOCK_STREAMS }),
      });
    });
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
  });

  test("rename input is present for each stream", async ({ page }) => {
    await page.goto("/");
    const renameInputs = page.locator('[data-testid="rename-input"]');
    await expect(renameInputs).toHaveCount(2);
  });

  test("rename button is present for each stream", async ({ page }) => {
    await page.goto("/");
    const renameBtns = page.locator('[data-testid="rename-btn"]');
    await expect(renameBtns).toHaveCount(2);
  });

  test("can fill in rename input and submit", async ({ page }) => {
    await page.goto("/");
    const firstInput = page.locator('[data-testid="rename-input"]').first();
    await firstInput.fill("Updated Alpha");
    const firstBtn = page.locator('[data-testid="rename-btn"]').first();
    await firstBtn.click();
    // After PATCH, the updated alias should appear
    await expect(page.getByText("Updated Alpha")).toBeVisible();
  });
});

test.describe("per-stream detail page", () => {
  test.beforeEach(async ({ page }) => {
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

  test("displays backlog metric", async ({ page }) => {
    await page.goto("/streams/stream-uuid-1");
    await expect(page.locator('[data-testid="metric-backlog"]')).toBeVisible();
    await expect(page.locator('[data-testid="metric-backlog"]')).toContainText(
      "3",
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
});
