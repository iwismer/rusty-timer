<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import {
    AlertBanner,
    Card,
    StatCard,
    StatusBadge,
  } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import type { AnnouncerRow, StatusResponse } from "$lib/api";

  let status = $state<StatusResponse | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(true);
  let poll: ReturnType<typeof setInterval> | undefined;

  let newestRows = $derived.by(() => {
    return [...(status?.announcer_rows ?? [])].sort(compareRowsNewestFirst);
  });

  function compareRowsNewestFirst(a: AnnouncerRow, b: AnnouncerRow) {
    return Date.parse(b.received_at) - Date.parse(a.received_at);
  }

  function formatReceivedAt(value: string) {
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return value;
    return date.toLocaleString();
  }

  function rowTime(row: AnnouncerRow) {
    return row.reader_timestamp || formatReceivedAt(row.received_at);
  }

  async function loadStatus() {
    try {
      status = await api.getStatus();
      error = null;
    } catch (err) {
      error = String(err);
    } finally {
      loading = false;
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

<div class="mx-auto px-4 py-6 space-y-6" style="max-width: 1100px;">
  <div class="flex flex-wrap items-center justify-between gap-3">
    <div>
      <h1 class="text-2xl font-bold text-text-primary m-0">Announcer</h1>
      <p class="text-sm text-text-muted mt-1 mb-0">
        Rolling finish list, newest reads first.
      </p>
    </div>
    {#if loading}
      <StatusBadge label="Loading" state="warn" />
    {:else if error}
      <StatusBadge label="Status stale" state="err" />
    {:else}
      <StatusBadge label="Live" state="ok" />
    {/if}
  </div>

  {#if error}
    <AlertBanner
      variant="err"
      message={`Could not refresh announcer rows: ${error}`}
    />
  {/if}

  <div class="grid gap-4 sm:grid-cols-2">
    <Card>
      <StatCard label="Finishers" value={status?.finisher_count ?? "—"} />
    </Card>
    <Card>
      <StatCard
        label="Announcer generation"
        value={status?.announcer_source_generation ?? "—"}
      />
    </Card>
  </div>

  <Card title="Finishers">
    {#if !status}
      <p class="text-sm text-text-muted m-0">Loading finishers…</p>
    {:else if newestRows.length === 0}
      <p class="text-sm text-text-muted m-0">No finishers announced yet.</p>
    {:else}
      <div class="overflow-x-auto">
        <table class="w-full border-collapse text-sm">
          <thead>
            <tr class="text-left text-text-muted border-b border-border">
              <th class="py-2 pr-4 font-medium">Time</th>
              <th class="py-2 pr-4 font-medium">Chip</th>
              <th class="py-2 pr-4 font-medium">Bib</th>
              <th class="py-2 pr-4 font-medium">Name</th>
              <th class="py-2 pr-4 font-medium">Stream</th>
              <th class="py-2 pr-4 font-medium">Seq</th>
            </tr>
          </thead>
          <tbody>
            {#each newestRows as row (`${row.stream_id}:${row.seq}`)}
              <tr class="border-b border-border last:border-0">
                <td class="py-2 pr-4 text-text-primary">{rowTime(row)}</td>
                <td class="py-2 pr-4 text-text-primary font-mono"
                  >{row.chip_id}</td
                >
                <td class="py-2 pr-4 text-text-primary font-mono"
                  >{row.bib ?? "—"}</td
                >
                <td class="py-2 pr-4 text-text-primary">{row.display_name}</td>
                <td class="py-2 pr-4 text-text-primary font-mono"
                  >{row.stream_id}</td
                >
                <td class="py-2 pr-4 text-text-primary font-mono">{row.seq}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </Card>
</div>
