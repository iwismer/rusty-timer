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

const MOCK_ALL_OFFLINE_STREAMS = [
  {
    stream_id: "stream-offline-1",
    forwarder_id: "fwd-offline",
    reader_ip: "172.16.0.10:10000",
    display_alias: "Offline Reader A",
    online: false,
    stream_epoch: 5,
    created_at: "2024-01-03T00:00:00Z",
  },
  {
    stream_id: "stream-offline-2",
    forwarder_id: "fwd-offline",
    reader_ip: "172.16.0.11:10000",
    display_alias: "Offline Reader B",
    online: false,
    stream_epoch: 6,
    created_at: "2024-01-04T00:00:00Z",
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

const RACE_ID = "11111111-1111-1111-1111-111111111111";
const MOCK_RACES_RESPONSE = {
  races: [
    {
      race_id: RACE_ID,
      name: "Des Moines 10k",
      date: "2026-05-01",
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    },
  ],
};

const MOCK_FORWARDER_RACES_RESPONSE = {
  assignments: [
    {
      forwarder_id: "fwd-alpha",
      race_id: null,
    },
  ],
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
    await page.route("**/api/v1/races", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(MOCK_RACES_RESPONSE),
      });
    });
    await page.route("**/api/v1/forwarder-races", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(MOCK_FORWARDER_RACES_RESPONSE),
      });
    });
  });

  test("renders page heading", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator('[data-testid="streams-heading"]')).toBeVisible();
  });

  test("renders announcer navigation link", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByRole("link", { name: "Announcer" })).toBeVisible();
  });

  test("renders list of streams", async ({ page }) => {
    await page.goto("/");
    await expect(
      page.locator('[data-testid="stream-list"]').first(),
    ).toBeVisible();
    const items = page.locator('[data-testid="stream-item"]');
    await expect(items).toHaveCount(2);
  });

  test("hide offline toggle filters offline stream rows", async ({ page }) => {
    await page.goto("/");

    const items = page.locator('[data-testid="stream-item"]');
    await expect(items).toHaveCount(2);

    await page.getByLabel("Hide offline").check();

    await expect(items).toHaveCount(1);
    await expect(
      page.locator('[data-testid="stream-offline-badge"]'),
    ).toHaveCount(0);
    await expect(
      page.locator('[data-testid="stream-online-badge"]'),
    ).toHaveCount(1);
  });

  test("hide offline preference persists after reload", async ({ page }) => {
    await page.goto("/");

    await page.getByLabel("Hide offline").check();
    await expect(page.locator('[data-testid="stream-item"]')).toHaveCount(1);

    await page.reload();

    await expect(page.getByLabel("Hide offline")).toBeChecked();
    await expect(page.locator('[data-testid="stream-item"]')).toHaveCount(1);
  });

  test("race filter preference persists after reload", async ({ page }) => {
    await page.route("**/api/v1/forwarder-races", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          assignments: [
            { forwarder_id: "fwd-alpha", race_id: RACE_ID },
            { forwarder_id: "fwd-beta", race_id: null },
          ],
        }),
      });
    });

    await page.addInitScript((raceId: string) => {
      localStorage.setItem("raceFilter", raceId);
    }, RACE_ID);

    await page.goto("/");
    await expect(
      page.locator('[data-testid="race-filter-select"]'),
    ).toHaveValue(RACE_ID);
    await expect(page.locator('[data-testid="stream-item"]')).toHaveCount(1);

    await page.reload();

    await expect(
      page.locator('[data-testid="race-filter-select"]'),
    ).toHaveValue(RACE_ID);
    await expect(page.locator('[data-testid="stream-item"]')).toHaveCount(1);
  });

  test("stale race filter preference resets to all races", async ({ page }) => {
    await page.addInitScript(() => {
      localStorage.setItem("raceFilter", "missing-race-id");
    });

    await page.goto("/");

    await expect(
      page.locator('[data-testid="race-filter-select"]'),
    ).toHaveValue("");
    await expect(page.locator('[data-testid="stream-item"]')).toHaveCount(2);
  });

  test("fails open to all races when races API is unavailable", async ({
    page,
  }) => {
    await page.route("**/api/v1/races", async (route) => {
      await route.fulfill({
        status: 500,
        contentType: "application/json",
        body: JSON.stringify({
          code: "INTERNAL_ERROR",
          message: "temporary failure",
        }),
      });
    });

    await page.addInitScript((raceId: string) => {
      localStorage.setItem("raceFilter", raceId);
    }, RACE_ID);

    await page.goto("/");

    await expect(
      page.locator('[data-testid="race-filter-select"]'),
    ).toHaveValue("");
    await expect(page.locator('[data-testid="stream-item"]')).toHaveCount(2);
  });

  test("shows empty state when selected race has no matching forwarders", async ({
    page,
  }) => {
    await page.route("**/api/v1/forwarder-races", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          assignments: [
            { forwarder_id: "fwd-alpha", race_id: null },
            { forwarder_id: "fwd-beta", race_id: null },
          ],
        }),
      });
    });

    await page.goto("/");
    await page
      .locator('[data-testid="race-filter-select"]')
      .selectOption(RACE_ID);

    await expect(page.locator('[data-testid="stream-item"]')).toHaveCount(0);
    await expect(
      page.getByText("No streams match the selected race."),
    ).toBeVisible();
  });

  test("shows race-specific empty state when hide offline removes all matching streams", async ({
    page,
  }) => {
    await page.route("**/api/v1/forwarder-races", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          assignments: [
            { forwarder_id: "fwd-alpha", race_id: null },
            { forwarder_id: "fwd-beta", race_id: RACE_ID },
          ],
        }),
      });
    });

    await page.goto("/");
    await page
      .locator('[data-testid="race-filter-select"]')
      .selectOption(RACE_ID);
    await expect(page.locator('[data-testid="stream-item"]')).toHaveCount(1);

    await page.getByLabel("Hide offline").check();

    await expect(page.locator('[data-testid="stream-item"]')).toHaveCount(0);
    await expect(
      page.getByText("No online streams match the selected race."),
    ).toBeVisible();
  });

  test("shows empty-state message when hiding offline streams with no online streams", async ({
    page,
  }) => {
    await page.route("**/api/v1/streams", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ streams: MOCK_ALL_OFFLINE_STREAMS }),
      });
    });

    await page.goto("/");
    await expect(page.locator('[data-testid="stream-item"]')).toHaveCount(2);

    await page.getByLabel("Hide offline").check();

    await expect(page.locator('[data-testid="stream-item"]')).toHaveCount(0);
    await expect(
      page.locator('[data-testid="no-online-streams"]'),
    ).toBeVisible();
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

  test("overview race selection is reflected on stream detail without SSE", async ({
    page,
  }) => {
    let assignedRaceId: string | null = null;

    await page.route("**/api/v1/forwarder-races", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          assignments: [
            {
              forwarder_id: "fwd-alpha",
              race_id: assignedRaceId,
            },
          ],
        }),
      });
    });

    await page.route("**/api/v1/forwarders/fwd-alpha/race", async (route) => {
      if (route.request().method() === "PUT") {
        const payload = route.request().postDataJSON() as {
          race_id?: string | null;
        };
        assignedRaceId = payload.race_id ?? null;
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            forwarder_id: "fwd-alpha",
            race_id: RACE_ID,
          }),
        });
        return;
      }
      await route.continue();
    });

    await page.goto("/");
    const putRequest = page.waitForRequest(
      (request) =>
        request.method() === "PUT" &&
        request.url().endsWith("/api/v1/forwarders/fwd-alpha/race"),
    );

    const raceSelect = page.locator(
      '[data-testid="forwarder-race-select-fwd-alpha"]',
    );
    await raceSelect.selectOption(RACE_ID);
    const request = await putRequest;
    expect(request.postDataJSON()).toEqual({ race_id: RACE_ID });

    await page.locator('[data-testid="stream-detail-link"]').first().click();
    await expect(page).toHaveURL("/streams/stream-uuid-1");
    await expect(page.getByRole("combobox").first()).toHaveValue(RACE_ID);
  });
});

test.describe("announcer public page", () => {
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
        body: JSON.stringify({ streams: [] }),
      });
    });
    await page.route("**/api/v1/races", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ races: [] }),
      });
    });
    await page.route("**/api/v1/forwarder-races", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ assignments: [] }),
      });
    });
    await page.route("**/api/v1/logs", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ entries: [] }),
      });
    });
    await page.route("**/api/v1/public/announcer/events", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: "event: keepalive\ndata: ok\n\n",
      });
    });
  });

  test("announcer-public disabled message renders", async ({ page }) => {
    await page.route("**/api/v1/public/announcer/state", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          public_enabled: false,
          finisher_count: 0,
          rows: [],
        }),
      });
    });

    await page.goto("/announcer");
    await expect(page.getByText("Announcer screen is disabled")).toBeVisible();
  });

  test("announcer-public enabled page shows disclaimer and rows", async ({
    page,
  }) => {
    await page.route("**/api/v1/public/announcer/state", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          public_enabled: true,
          finisher_count: 1,
          rows: [
            {
              announcement_id: 1,
              bib: 333,
              display_name: "Runner Three",
              reader_timestamp: "10:00:00",
            },
          ],
        }),
      });
    });

    await page.goto("/announcer");
    await expect(page.getByText("Runner Three")).toBeVisible();
    await expect(page.getByText(/not official results/i)).toBeVisible();
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
    await page.route("**/api/v1/races", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(MOCK_RACES_RESPONSE),
      });
    });
    await page.route("**/api/v1/forwarder-races", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(MOCK_FORWARDER_RACES_RESPONSE),
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

  test("stream detail race selection is reflected on overview without SSE", async ({
    page,
  }) => {
    let assignedRaceId: string | null = null;

    await page.route("**/api/v1/forwarder-races", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          assignments: [
            {
              forwarder_id: "fwd-alpha",
              race_id: assignedRaceId,
            },
          ],
        }),
      });
    });

    await page.route("**/api/v1/forwarders/fwd-alpha/race", async (route) => {
      if (route.request().method() === "PUT") {
        const payload = route.request().postDataJSON() as {
          race_id?: string | null;
        };
        assignedRaceId = payload.race_id ?? null;
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            forwarder_id: "fwd-alpha",
            race_id: RACE_ID,
          }),
        });
        return;
      }
      await route.continue();
    });

    await page.goto("/streams/stream-uuid-1");
    const putRequest = page.waitForRequest(
      (request) =>
        request.method() === "PUT" &&
        request.url().endsWith("/api/v1/forwarders/fwd-alpha/race"),
    );

    const raceSelect = page.getByRole("combobox").first();
    await raceSelect.selectOption(RACE_ID);
    const request = await putRequest;
    expect(request.postDataJSON()).toEqual({ race_id: RACE_ID });

    await page.goto("/");
    await expect(page.locator('[data-testid="streams-heading"]')).toBeVisible();
    await expect(
      page.locator('[data-testid="forwarder-race-select-fwd-alpha"]'),
    ).toHaveValue(RACE_ID);
  });
});

test.describe("forwarder reads page", () => {
  test.beforeEach(async ({ page }) => {
    await page.route("**/api/v1/events", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: "event: keepalive\ndata: ok\n\n",
      });
    });
    await page.route("**/api/v1/races", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(MOCK_RACES_RESPONSE),
      });
    });
    await page.route("**/api/v1/forwarder-races", async (route) => {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(MOCK_FORWARDER_RACES_RESPONSE),
      });
    });
  });

  test("ignores stale response when sort is toggled quickly", async ({
    page,
  }) => {
    await page.route(
      "**/api/v1/forwarders/fwd-alpha/reads**",
      async (route) => {
        const url = new URL(route.request().url());
        const order = url.searchParams.get("order");
        if (order === "asc") {
          await route.fulfill({
            status: 200,
            contentType: "application/json",
            body: JSON.stringify({
              reads: [
                {
                  stream_id: "stream-uuid-1",
                  seq: 10,
                  reader_timestamp: "2026-01-01T00:00:10Z",
                  tag_id: "chip-asc",
                  received_at: "2026-01-01T00:00:10Z",
                  bib: 11,
                  first_name: "Asc",
                  last_name: "Winner",
                },
              ],
              total: 1,
              limit: 100,
              offset: 0,
            }),
          });
          return;
        }

        await page.waitForTimeout(300);
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            reads: [
              {
                stream_id: "stream-uuid-1",
                seq: 20,
                reader_timestamp: "2026-01-01T00:00:20Z",
                tag_id: "chip-desc",
                received_at: "2026-01-01T00:00:20Z",
                bib: 22,
                first_name: "Desc",
                last_name: "Stale",
              },
            ],
            total: 1,
            limit: 100,
            offset: 0,
          }),
        });
      },
    );

    await page.goto("/forwarders/fwd-alpha/reads");
    const orderBtn = page.getByRole("button", { name: "Newest first" });
    await expect(orderBtn).toBeVisible();
    await orderBtn.click();

    await page.waitForTimeout(450);
    await expect(page.getByText("Asc Winner (#11)")).toBeVisible();
    await expect(page.getByText("Desc Stale (#22)")).toHaveCount(0);
  });
});
