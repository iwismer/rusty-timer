<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { getAnnouncerState } from "$lib/api";
  import type { AnnouncerDelta, AnnouncerRow } from "$lib/api";
  import { connectAnnouncerEvents } from "$lib/announcer-client";

  let loading = $state(true);
  let loadError: string | null = $state(null);
  let publicEnabled = $state(false);
  let finisherCount = $state(0);
  let rows: AnnouncerRow[] = $state([]);
  let maxListSize = $state(25);
  let flashRowKeys = $state(new Set<string>());

  let eventSource: EventSource | null = null;

  onMount(() => {
    void loadSnapshot();
    eventSource = connectAnnouncerEvents({
      onUpdate: applyDelta,
      onResync: () => {
        void loadSnapshot();
      },
    });
  });

  onDestroy(() => {
    eventSource?.close();
    eventSource = null;
  });

  async function loadSnapshot() {
    loading = true;
    loadError = null;
    try {
      const state = await getAnnouncerState();
      publicEnabled = state.public_enabled;
      finisherCount = state.finisher_count;
      rows = [...state.rows];
      maxListSize = state.max_list_size;
    } catch (err) {
      loadError = String(err);
    } finally {
      loading = false;
    }
  }

  function applyDelta(delta: AnnouncerDelta) {
    const key = rowKey(delta.row);
    const deduped = rows.filter((row) => rowKey(row) !== key);
    rows = [delta.row, ...deduped].slice(0, maxListSize);
    finisherCount = delta.finisher_count;
    markRowFlash(key);
  }

  function markRowFlash(key: string) {
    const next = new Set(flashRowKeys);
    next.add(key);
    flashRowKeys = next;
    setTimeout(() => {
      const updated = new Set(flashRowKeys);
      updated.delete(key);
      flashRowKeys = updated;
    }, 1200);
  }

  function rowKey(row: AnnouncerRow): string {
    return `${row.stream_id}:${row.seq}`;
  }
</script>

<main class="min-h-screen bg-surface-0 text-text-primary px-6 py-8">
  {#if loading}
    <p class="text-sm text-text-muted">Loading announcer feed...</p>
  {:else if loadError}
    <p class="text-sm text-status-err">{loadError}</p>
  {:else if !publicEnabled}
    <section class="max-w-[900px] mx-auto text-center py-20">
      <h1 class="text-3xl font-bold m-0 mb-4">Announcer screen is disabled</h1>
      <p class="text-sm text-text-muted m-0">
        Ask an operator to enable announcer mode from the dashboard.
      </p>
    </section>
  {:else}
    <section class="max-w-[1100px] mx-auto">
      <div
        class="rounded-md border border-status-warn-border bg-status-warn-bg px-4 py-3 mb-5"
      >
        <p class="text-sm text-status-warn m-0">
          Not official results. Times and places are announcer assist only.
        </p>
      </div>

      <h1 class="text-3xl font-bold m-0 mb-4">Announcer Feed</h1>
      <p class="text-sm text-text-muted mt-0 mb-4">
        Newest finishers at the top.
      </p>

      {#if rows.length === 0}
        <p class="text-sm text-text-muted">Waiting for first finisher...</p>
      {:else}
        <ul class="list-none p-0 m-0 grid gap-3">
          {#each rows as row (rowKey(row))}
            <li
              data-testid={"announcer-row-" + rowKey(row)}
              class={[
                "rounded-md border border-border bg-surface-1 p-4",
                flashRowKeys.has(rowKey(row)) ? "flash-new" : "",
              ]
                .join(" ")
                .trim()}
            >
              <div class="flex items-center justify-between gap-3">
                <p class="text-lg font-semibold m-0">{row.display_name}</p>
                {#if row.bib !== null}
                  <p class="text-sm text-text-muted m-0">Bib {row.bib}</p>
                {/if}
              </div>
              {#if row.reader_timestamp}
                <p class="text-sm text-text-muted mt-2 mb-0">
                  Time {row.reader_timestamp}
                </p>
              {/if}
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
