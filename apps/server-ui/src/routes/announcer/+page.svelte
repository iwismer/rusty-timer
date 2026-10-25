<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { AlertBanner, buttonClass } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import type { AnnouncerRow, StatusResponse } from "$lib/api";

  let status = $state<StatusResponse | null>(null);
  let loadError = $state<string | null>(null);
  let loading = $state(true);
  let poll: ReturnType<typeof setInterval> | undefined;

  let resetting = $state(false);
  let confirmingReset = $state(false);
  let resetError = $state<string | null>(null);
  let resetSuccess = $state<string | null>(null);
  let lastGeneration: number | undefined;

  // Keys that arrived since the last poll, used to flash new rows. Seeded on
  // the first successful load so an initial backlog does not all flash at once.
  let seenKeys = new Set<string>();
  let seeded = false;
  let flashKeys = $state(new Set<string>());

  let rows = $derived.by(() =>
    [...(status?.announcer_rows ?? [])].sort(compareRowsNewestFirst),
  );
  let finisherCount = $derived(status?.finisher_count ?? 0);

  function compareRowsNewestFirst(a: AnnouncerRow, b: AnnouncerRow) {
    return Date.parse(b.received_at) - Date.parse(a.received_at);
  }

  function rowKey(row: AnnouncerRow): string {
    // Composite stream identity: the wire stream_id alone is ambiguous when
    // two forwarders expose the same stream id.
    return `${row.forwarder_endpoint_id}:${row.stream_id}:${row.seq}`;
  }

  function rowTime(row: AnnouncerRow): string {
    if (row.reader_timestamp) return row.reader_timestamp;
    const date = new Date(row.received_at);
    return Number.isNaN(date.getTime())
      ? row.received_at
      : date.toLocaleTimeString();
  }

  function markFlash(key: string) {
    const next = new Set(flashKeys);
    next.add(key);
    flashKeys = next;
    setTimeout(() => {
      const updated = new Set(flashKeys);
      updated.delete(key);
      flashKeys = updated;
    }, 1200);
  }

  function reconcileNewRows(current: AnnouncerRow[]) {
    if (!seeded) {
      for (const row of current) seenKeys.add(rowKey(row));
      seeded = true;
      return;
    }
    for (const row of current) {
      const key = rowKey(row);
      if (!seenKeys.has(key)) {
        seenKeys.add(key);
        markFlash(key);
      }
    }
  }

  async function loadStatus() {
    try {
      const next = await api.getStatus();
      if (
        lastGeneration !== undefined &&
        next.announcer_source_generation !== lastGeneration
      ) {
        // Generation advanced remotely (e.g. from receiver or another tab).
        seenKeys.clear();
        seeded = false;
        flashKeys = new Set();
      }
      lastGeneration = next.announcer_source_generation;
      status = next;
      loadError = null;
      reconcileNewRows(next.announcer_rows ?? []);
    } catch (err) {
      loadError = String(err);
    } finally {
      loading = false;
    }
  }

  async function handleReset() {
    confirmingReset = false;
    resetting = true;
    resetError = null;
    resetSuccess = null;
    try {
      const res = await api.resetAnnouncer();
      lastGeneration = res.announcer_source_generation;
      seenKeys.clear();
      seeded = false;
      flashKeys = new Set();
      if (status) {
        status = {
          ...status,
          announcer_source_generation: res.announcer_source_generation,
          finisher_count: 0,
          announcer_rows: [],
        };
      }
      resetSuccess = "Announcer feed reset for the next race.";
      setTimeout(() => {
        resetSuccess = null;
      }, 4000);
    } catch (err) {
      resetError = String(err);
    } finally {
      resetting = false;
    }
  }

  onMount(() => {
    void loadStatus();
    poll = setInterval(() => void loadStatus(), 2_000);
  });

  onDestroy(() => {
    if (poll) clearInterval(poll);
  });
</script>

<svelte:head>
  <title>Announcer Feed · Rusty Timer</title>
</svelte:head>

<main class="min-h-screen bg-surface-0 text-text-primary px-6 py-8">
  {#if loading}
    <p class="text-sm text-text-muted">Loading announcer feed…</p>
  {:else}
    <section class="max-w-[1100px] mx-auto">
      <div
        class="rounded-md border border-status-warn-border bg-status-warn-bg px-4 py-3 mb-5"
      >
        <p class="text-sm text-status-warn m-0">
          Not official results. Times and places are announcer assist only.
        </p>
      </div>

      <div class="flex flex-wrap items-center justify-between gap-4 mb-4">
        <div>
          <h1 class="text-3xl font-bold m-0 mb-1">Announcer Feed</h1>
          <p class="text-sm text-text-muted mt-0 mb-0">
            Newest finishers at the top.
          </p>
        </div>

        <div>
          {#if confirmingReset}
            <div class="flex flex-wrap items-center gap-2">
              <span class="text-xs text-status-warn font-medium">
                Reset feed for next race?
              </span>
              <button
                data-testid="confirm-announcer-reset-btn"
                type="button"
                class={buttonClass("danger-soft", "sm")}
                disabled={resetting}
                onclick={() => void handleReset()}
              >
                {resetting ? "Resetting…" : "Yes, Reset Feed"}
              </button>
              <button
                type="button"
                class={buttonClass("secondary", "sm")}
                disabled={resetting}
                onclick={() => (confirmingReset = false)}
              >
                Cancel
              </button>
            </div>
          {:else}
            <button
              data-testid="announcer-reset-btn"
              type="button"
              class={buttonClass("secondary", "sm")}
              disabled={resetting}
              onclick={() => (confirmingReset = true)}
            >
              Reset for Next Race…
            </button>
          {/if}
        </div>
      </div>

      {#if resetSuccess}
        <div class="mb-4">
          <AlertBanner
            variant="ok"
            message={resetSuccess}
            onDismiss={() => (resetSuccess = null)}
          />
        </div>
      {/if}
      {#if resetError}
        <div class="mb-4">
          <AlertBanner
            variant="err"
            message={resetError}
            onDismiss={() => (resetError = null)}
          />
        </div>
      {/if}

      {#if loadError}
        <p
          data-testid="announcer-load-error"
          class="text-sm text-status-err mb-4"
        >
          Could not refresh announcer feed: {loadError}
        </p>
      {/if}

      {#if rows.length === 0}
        <p class="text-sm text-text-muted">Waiting for first finisher…</p>
      {:else}
        <ul class="list-none p-0 m-0 grid gap-3">
          {#each rows as row (rowKey(row))}
            <li
              data-testid={"announcer-row-" + rowKey(row)}
              class={[
                "rounded-md border border-border bg-surface-1 p-4",
                flashKeys.has(rowKey(row)) ? "flash-new" : "",
              ]
                .join(" ")
                .trim()}
            >
              <div class="flex items-center justify-between gap-3">
                <p class="text-lg font-semibold m-0">
                  {row.display_name || "Unknown runner"}
                </p>
                {#if row.bib !== null && row.bib !== undefined}
                  <p class="text-sm text-text-muted m-0">Bib {row.bib}</p>
                {/if}
              </div>
              <p class="text-sm text-text-muted mt-2 mb-0">
                Time {rowTime(row)}
              </p>
            </li>
          {/each}
        </ul>
      {/if}

      <footer class="mt-6 border-t border-border pt-4">
        <p class="text-base font-medium m-0">
          Finishers announced: {finisherCount}
        </p>
      </footer>
    </section>
  {/if}
</main>

<style>
  .flash-new {
    animation: announcer-flash 1.2s ease-out;
    border-color: var(--status-ok-border, #a7f3d0);
  }

  @keyframes announcer-flash {
    0% {
      background-color: color-mix(
        in srgb,
        var(--status-ok-bg, #ecfdf5) 85%,
        white
      );
    }
    100% {
      background-color: transparent;
    }
  }
</style>
