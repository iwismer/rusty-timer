<script lang="ts">
  import { onDestroy } from "svelte";
  import { page } from "$app/stores";
  import * as api from "$lib/api";
  import type { ReadEntry, DedupMode, SortOrder } from "$lib/api";
  import {
    streamsStore,
    metricsStore,
    setMetrics,
    forwarderRacesStore,
    racesStore,
  } from "$lib/stores";
  import { shouldFetchMetrics } from "$lib/streamMetricsLoader";
  import { StatusBadge, Card } from "@rusty-timer/shared-ui";
  import ReadsTable from "$lib/components/ReadsTable.svelte";

  let resetResult: string | null = $state(null);
  let resetBusy = $state(false);
  let renameValue = $state("");
  let renameBusy = $state(false);
  let renameError: string | null = $state(null);
  let requestedMetricStreamIds = $state(new Set<string>());
  let inFlightMetricStreamIds = $state(new Set<string>());

  // Reads state
  let reads: ReadEntry[] = $state([]);
  let readsTotal = $state(0);
  let readsLoading = $state(false);
  let readsDedup: DedupMode = $state("none");
  let readsWindowSecs = $state(5);
  let readsLimit = $state(100);
  let readsOffset = $state(0);
  let readsOrder: SortOrder = $state("desc");

  let streamId = $derived($page.params.streamId!);
  let stream = $derived(
    $streamsStore.find((s) => s.stream_id === streamId) ?? null,
  );
  let metrics = $derived($metricsStore[streamId] ?? null);

  // Keep rename input in sync when stream data arrives
  $effect(() => {
    if (stream && renameValue === "") {
      renameValue = stream.display_alias ?? "";
    }
  });

  $effect(() => {
    void maybeFetchMetrics(streamId);
  });

  function maybeFetchMetrics(id: string): void {
    if (
      !shouldFetchMetrics(
        id,
        $metricsStore,
        requestedMetricStreamIds,
        inFlightMetricStreamIds,
      )
    ) {
      return;
    }

    requestedMetricStreamIds = new Set(requestedMetricStreamIds).add(id);
    inFlightMetricStreamIds = new Set(inFlightMetricStreamIds).add(id);
    void loadMetrics(id);
  }

  async function loadMetrics(id: string): Promise<void> {
    try {
      const m = await api.getMetrics(id);
      setMetrics(id, m);
    } catch {
      // SSE will populate eventually.
    } finally {
      const next = new Set(inFlightMetricStreamIds);
      next.delete(id);
      inFlightMetricStreamIds = next;
    }
  }

  async function handleResetEpoch() {
    resetBusy = true;
    resetResult = null;
    try {
      await api.resetEpoch(streamId);
      resetResult = "Epoch reset command sent successfully.";
    } catch (e) {
      resetResult = `Error: ${String(e)}`;
    } finally {
      resetBusy = false;
    }
  }

  function formatLag(lag: number | null): string {
    if (lag === null) return "N/A (no events yet)";
    if (lag < 1000) return `${lag} ms`;
    return `${(lag / 1000).toFixed(1)} s`;
  }

  let timeSinceLastRead: string = $state("N/A");

  function formatDuration(ms: number): string {
    if (ms < 1000) return "< 1s";
    const totalSec = Math.floor(ms / 1000);
    const hours = Math.floor(totalSec / 3600);
    const minutes = Math.floor((totalSec % 3600) / 60);
    const seconds = totalSec % 60;
    if (hours > 0) return `${hours}h ${minutes}m ${seconds}s`;
    if (minutes > 0) return `${minutes}m ${seconds}s`;
    return `${seconds}s`;
  }

  function updateTimeSinceLastRead(): void {
    if (metrics?.epoch_last_received_at) {
      const elapsed =
        Date.now() - new Date(metrics.epoch_last_received_at).getTime();
      timeSinceLastRead = formatDuration(Math.max(0, elapsed));
    } else {
      timeSinceLastRead = "N/A (no events in epoch)";
    }
  }

  // Set up interval for time-since-last-read, with cleanup on re-run and destroy
  $effect(() => {
    updateTimeSinceLastRead();
    const handle = setInterval(updateTimeSinceLastRead, 1000);
    return () => {
      clearInterval(handle);
    };
  });

  // Load reads on mount and re-fetch when new data arrives (metrics update via SSE)
  let readsInitialized = false;
  $effect(() => {
    metrics; // re-run when metrics change (signals new reads arrived)
    void loadReads(streamId, readsInitialized);
    readsInitialized = true;
  });

  async function loadReads(id: string, silent = false): Promise<void> {
    if (!silent) readsLoading = true;
    try {
      const resp = await api.getStreamReads(id, {
        dedup: readsDedup,
        window_secs: readsWindowSecs,
        limit: readsLimit,
        offset: readsOffset,
        order: readsOrder,
      });
      reads = resp.reads;
      readsTotal = resp.total;
    } catch {
      reads = [];
      readsTotal = 0;
    } finally {
      readsLoading = false;
    }
  }

  async function handleRename() {
    renameBusy = true;
    renameError = null;
    try {
      await api.renameStream(streamId, renameValue);
    } catch (e) {
      renameError = String(e);
    } finally {
      renameBusy = false;
    }
  }

  async function handleRaceChange(forwarderId: string, raceId: string | null) {
    try {
      await api.setForwarderRace(forwarderId, raceId);
    } catch {
      // SSE will correct any stale state
    }
  }

  function handleReadsParamsChange() {
    void loadReads(streamId, true);
  }
</script>

<main class="max-w-[1100px] mx-auto px-6 py-6">
  <div class="mb-4">
    <a
      data-testid="back-link"
      href="/"
      class="text-xs text-accent no-underline hover:underline"
    >
      &larr; Back to stream list
    </a>
  </div>

  <div class="flex items-center gap-3 mb-6">
    <h1
      data-testid="stream-detail-heading"
      class="text-xl font-bold text-text-primary m-0"
    >
      {#if stream}
        {stream.display_alias ?? `${stream.forwarder_id} / ${stream.reader_ip}`}
      {:else}
        {streamId}
      {/if}
    </h1>
    {#if stream}
      <StatusBadge
        label={stream.online ? "online" : "offline"}
        state={stream.online ? "ok" : "err"}
      />
    {/if}
  </div>

  {#if stream}
    <div class="grid grid-cols-2 gap-4 mb-6">
      <Card title="Info">
        <dl
          class="grid gap-y-2 gap-x-4 text-sm m-0"
          style="grid-template-columns: auto 1fr;"
        >
          <dt class="text-text-muted">Stream ID</dt>
          <dd class="font-mono text-text-primary m-0">{stream.stream_id}</dd>
          <dt class="text-text-muted">Forwarder</dt>
          <dd class="text-text-primary m-0">{stream.forwarder_id}</dd>
          <dt class="text-text-muted">Race</dt>
          <dd class="m-0">
            <select
              class="text-xs px-2 py-1 rounded-md border border-border bg-surface-0 text-text-primary"
              value={$forwarderRacesStore[stream.forwarder_id] ?? ""}
              onchange={(e) => {
                const val = e.currentTarget.value;
                handleRaceChange(stream.forwarder_id, val || null);
              }}
            >
              <option value="">No race</option>
              {#each $racesStore as race (race.race_id)}
                <option value={race.race_id}>{race.name}</option>
              {/each}
            </select>
          </dd>
          <dt class="text-text-muted">Reader IP</dt>
          <dd class="font-mono text-text-primary m-0">{stream.reader_ip}</dd>
          <dt class="text-text-muted">Epoch</dt>
          <dd class="font-mono text-text-primary m-0">
            {stream.stream_epoch}
          </dd>
          <dt class="text-text-muted">Created</dt>
          <dd class="text-text-primary m-0">
            {new Date(stream.created_at).toLocaleString()}
          </dd>
        </dl>

        <div class="mt-3 pt-3 border-t border-border">
          <p class="text-xs font-medium text-text-muted mb-2 m-0">
            Display Alias
          </p>
          <div class="flex gap-2 items-center">
            <input
              data-testid="rename-input"
              type="text"
              bind:value={renameValue}
              placeholder="Display alias"
              aria-label="Rename stream {streamId}"
              class="flex-1 px-2 py-1 text-sm rounded-md border border-border bg-surface-0 text-text-primary placeholder:text-text-muted focus:outline-none focus:border-accent"
            />
            <button
              data-testid="rename-btn"
              onclick={handleRename}
              disabled={renameBusy}
              class="px-3 py-1 text-xs font-medium rounded-md bg-surface-2 border border-border text-text-secondary cursor-pointer hover:bg-surface-3 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {renameBusy ? "Saving…" : "Rename"}
            </button>
          </div>
          {#if renameError}
            <p class="text-xs text-status-err mt-1 m-0">
              {renameError}
            </p>
          {/if}
        </div>
      </Card>

      <Card title="Metrics">
        <div data-testid="metrics-section">
          {#if !metrics}
            <p class="text-sm text-text-muted italic m-0">Loading metrics…</p>
          {:else}
            <dl
              class="grid gap-y-2 gap-x-4 text-sm m-0"
              style="grid-template-columns: auto 1fr;"
            >
              <dt class="text-text-muted">Raw count</dt>
              <dd
                data-testid="metric-raw-count"
                class="font-mono text-text-primary text-right m-0"
              >
                {metrics.raw_count.toLocaleString()}
              </dd>
              <dt class="text-text-muted">Dedup count</dt>
              <dd
                data-testid="metric-dedup-count"
                class="font-mono text-text-primary text-right m-0"
              >
                {metrics.dedup_count.toLocaleString()}
              </dd>
              <dt class="text-text-muted">Retransmit</dt>
              <dd
                data-testid="metric-retransmit-count"
                class="font-mono text-text-primary text-right m-0"
              >
                {metrics.retransmit_count.toLocaleString()}
              </dd>
              <dt class="text-text-muted">Lag</dt>
              <dd
                data-testid="metric-lag"
                class="font-mono text-text-primary text-right m-0"
              >
                {formatLag(metrics.lag)}
              </dd>
              <dt class="text-text-muted">Backlog</dt>
              <dd
                data-testid="metric-backlog"
                class="font-mono text-text-primary text-right m-0"
              >
                {metrics.backlog}
              </dd>
            </dl>

            <div class="mt-3 pt-3 border-t border-border">
              <p class="text-xs font-medium text-text-muted mb-2 m-0">
                Current Epoch
              </p>
              <dl
                class="grid gap-y-2 gap-x-4 text-sm m-0"
                style="grid-template-columns: auto 1fr;"
              >
                <dt class="text-text-muted">Raw (epoch)</dt>
                <dd
                  data-testid="metric-epoch-raw-count"
                  class="font-mono text-text-primary text-right m-0"
                >
                  {metrics.epoch_raw_count.toLocaleString()}
                </dd>
                <dt class="text-text-muted">Dedup (epoch)</dt>
                <dd
                  data-testid="metric-epoch-dedup-count"
                  class="font-mono text-text-primary text-right m-0"
                >
                  {metrics.epoch_dedup_count.toLocaleString()}
                </dd>
                <dt class="text-text-muted">Retransmit (epoch)</dt>
                <dd
                  data-testid="metric-epoch-retransmit-count"
                  class="font-mono text-text-primary text-right m-0"
                >
                  {metrics.epoch_retransmit_count.toLocaleString()}
                </dd>
                <dt class="text-text-muted">Unique chips</dt>
                <dd
                  data-testid="metric-unique-chips"
                  class="font-mono text-text-primary text-right m-0"
                >
                  {metrics.unique_chips.toLocaleString()}
                </dd>
                <dt class="text-text-muted">Last read</dt>
                <dd
                  data-testid="metric-last-read"
                  class="text-text-primary text-right m-0"
                >
                  {metrics.epoch_last_received_at
                    ? new Date(metrics.epoch_last_received_at).toLocaleString()
                    : "N/A (no events in epoch)"}
                </dd>
                <dt class="text-text-muted">Time since last read</dt>
                <dd
                  data-testid="metric-time-since-last-read"
                  class="text-text-primary text-right m-0"
                >
                  {timeSinceLastRead}
                </dd>
              </dl>
            </div>
          {/if}
        </div>
      </Card>
    </div>

    <div class="grid grid-cols-2 gap-4 mb-6">
      <Card title="Export">
        <div data-testid="export-section">
          <p class="text-xs text-text-secondary mb-3 m-0">
            All canonical (deduplicated) events, ordered by epoch and sequence.
          </p>
          <div class="flex flex-col gap-2">
            <a
              data-testid="export-raw-link"
              href={api.exportRawUrl(streamId)}
              download
              class="text-sm text-accent no-underline hover:underline"
            >
              Download export.txt
            </a>
            <a
              data-testid="export-csv-link"
              href={api.exportCsvUrl(streamId)}
              download
              class="text-sm text-accent no-underline hover:underline"
            >
              Download export.csv
            </a>
          </div>
        </div>
      </Card>

      <Card title="Actions">
        <button
          data-testid="reset-epoch-btn"
          onclick={handleResetEpoch}
          disabled={resetBusy}
          class="px-3 py-1.5 text-sm font-medium rounded-md bg-status-err-bg text-status-err border border-status-err-border cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {resetBusy ? "Sending…" : "Reset Epoch"}
        </button>
        <p class="text-xs text-text-muted mt-2 m-0">
          Sends an epoch-reset command to the connected forwarder. Only works
          while the forwarder is connected; returns 409 otherwise.
        </p>
        {#if resetResult}
          <p
            data-testid="reset-epoch-result"
            class="text-sm mt-2 m-0 {resetResult.startsWith('Error')
              ? 'text-status-err'
              : 'text-status-ok'}"
          >
            {resetResult}
          </p>
        {/if}
      </Card>
    </div>

    <div class="mb-6">
      <Card title="Reads">
        <ReadsTable
          {reads}
          total={readsTotal}
          loading={readsLoading}
          bind:dedup={readsDedup}
          bind:windowSecs={readsWindowSecs}
          bind:limit={readsLimit}
          bind:offset={readsOffset}
          bind:order={readsOrder}
          onParamsChange={handleReadsParamsChange}
        />
      </Card>
    </div>
  {:else}
    <p class="text-sm text-text-muted">
      Stream not found. It may appear once the forwarder connects.
    </p>
  {/if}
</main>
