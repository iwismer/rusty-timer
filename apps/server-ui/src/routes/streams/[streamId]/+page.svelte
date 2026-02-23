<script lang="ts">
  import { page } from "$app/stores";
  import * as api from "$lib/api";
  import { onStreamUpdated } from "$lib/sse";
  import type { ReadEntry, DedupMode, SortOrder } from "$lib/api";
  import {
    streamsStore,
    metricsStore,
    setMetrics,
    forwarderRacesStore,
    racesStore,
    setForwarderRace,
  } from "$lib/stores";
  import { shouldFetchMetrics } from "$lib/streamMetricsLoader";
  import { StatusBadge, Card } from "@rusty-timer/shared-ui";
  import ReadsTable from "$lib/components/ReadsTable.svelte";
  import { createLatestRequestGate } from "$lib/latestRequestGate";

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
  const readsRequestGate = createLatestRequestGate();
  type EpochRaceRow = {
    epoch: number;
    event_count: number;
    is_current: boolean;
    selected_race_id: string | null;
    saved_race_id: string | null;
    selected_name: string;
    saved_name: string | null;
    pending: boolean;
    status: "saved" | "error" | "incomplete";
  };
  let epochRaceRows = $state<EpochRaceRow[]>([]);
  let epochRaceRowsLoading = $state(false);
  let epochRaceRowsError: string | null = $state(null);
  let epochRaceRowsHydrationIncomplete = $state(false);
  let epochAdvancePending = $state(false);
  let epochAdvanceStatus: "idle" | "error" = $state("idle");
  let epochAdvanceAwaitingReload = $state(false);
  let epochRaceLoadVersion = 0;

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

  $effect(() => {
    void loadEpochRaceRows(streamId, $racesStore);
  });

  $effect(() => {
    const unsubscribe = onStreamUpdated((update) => {
      if (update.stream_id !== streamId) return;
      if (typeof update.stream_epoch !== "number") return;
      void loadEpochRaceRows(streamId, $racesStore);
    });
    return () => {
      unsubscribe();
    };
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
    const token = readsRequestGate.next();
    if (!silent) readsLoading = true;
    try {
      const resp = await api.getStreamReads(id, {
        dedup: readsDedup,
        window_secs: readsWindowSecs,
        limit: readsLimit,
        offset: readsOffset,
        order: readsOrder,
      });
      if (!readsRequestGate.isLatest(token)) return;
      reads = resp.reads;
      readsTotal = resp.total;
    } catch {
      if (!readsRequestGate.isLatest(token)) return;
      reads = [];
      readsTotal = 0;
    } finally {
      if (!readsRequestGate.isLatest(token)) return;
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
    const previousRaceId = $forwarderRacesStore[forwarderId] ?? null;
    setForwarderRace(forwarderId, raceId);
    try {
      await api.setForwarderRace(forwarderId, raceId);
    } catch {
      setForwarderRace(forwarderId, previousRaceId);
    }
  }

  function handleReadsParamsChange() {
    void loadReads(streamId, true);
  }

  async function loadEpochRaceRows(
    id: string,
    races: typeof $racesStore,
  ): Promise<void> {
    const currentVersion = ++epochRaceLoadVersion;
    epochRaceRowsLoading = true;
    epochRaceRowsError = null;
    epochRaceRowsHydrationIncomplete = false;

    try {
      const epochs = await api.getStreamEpochs(id);
      const savedRaceByEpoch = new Map<number, string | null>(
        epochs.map((epochInfo) => [epochInfo.epoch, null]),
      );

      let hasMappingFetchFailures = false;
      if (races.length > 0) {
        const mappingResponses = await Promise.allSettled(
          races.map((race) => api.getRaceStreamEpochMappings(race.race_id)),
        );

        for (const response of mappingResponses) {
          if (response.status !== "fulfilled") {
            hasMappingFetchFailures = true;
            continue;
          }
          for (const mapping of response.value.mappings) {
            if (mapping.stream_id !== id) continue;
            if (!savedRaceByEpoch.has(mapping.stream_epoch)) continue;
            savedRaceByEpoch.set(mapping.stream_epoch, mapping.race_id);
          }
        }
      }

      if (currentVersion !== epochRaceLoadVersion) return;
      epochRaceRowsHydrationIncomplete = hasMappingFetchFailures;

      epochRaceRows = epochs.map((epochInfo) => {
        const savedRaceId = savedRaceByEpoch.get(epochInfo.epoch) ?? null;
        const normalizedSavedName = normalizeEpochName(epochInfo.name ?? "");
        return {
          epoch: epochInfo.epoch,
          event_count: epochInfo.event_count,
          is_current: epochInfo.is_current,
          selected_race_id: savedRaceId,
          saved_race_id: savedRaceId,
          selected_name: normalizedSavedName ?? "",
          saved_name: normalizedSavedName,
          pending: false,
          status: hasMappingFetchFailures ? "incomplete" : "saved",
        };
      });
    } catch (e) {
      if (currentVersion !== epochRaceLoadVersion) return;
      epochRaceRows = [];
      epochRaceRowsError = String(e);
      epochRaceRowsHydrationIncomplete = false;
    } finally {
      if (currentVersion === epochRaceLoadVersion) {
        epochRaceRowsLoading = false;
        if (epochAdvanceAwaitingReload) {
          epochAdvanceAwaitingReload = false;
          epochAdvancePending = false;
        }
      }
    }
  }

  function normalizeEpochName(name: string): string | null {
    const trimmed = name.trim();
    return trimmed === "" ? null : trimmed;
  }

  function isEpochRowDirty(row: EpochRaceRow): boolean {
    return (
      row.selected_race_id !== row.saved_race_id ||
      normalizeEpochName(row.selected_name) !==
        normalizeEpochName(row.saved_name ?? "")
    );
  }

  function updateEpochRaceRow(
    epoch: number,
    update: (row: EpochRaceRow) => EpochRaceRow,
  ): void {
    epochRaceRows = epochRaceRows.map((row) =>
      row.epoch === epoch ? update(row) : row,
    );
  }

  function onEpochRaceSelectChange(epoch: number, raceId: string | null): void {
    epochAdvanceStatus = "idle";
    updateEpochRaceRow(epoch, (row) => ({
      ...row,
      selected_race_id: raceId,
      status: row.status === "incomplete" ? "incomplete" : "saved",
    }));
  }

  function onEpochNameInput(epoch: number, name: string): void {
    epochAdvanceStatus = "idle";
    updateEpochRaceRow(epoch, (row) => ({
      ...row,
      selected_name: name,
      status: row.status === "incomplete" ? "incomplete" : "saved",
    }));
  }

  async function handleSaveEpochRace(epoch: number): Promise<void> {
    const row = epochRaceRows.find((candidate) => candidate.epoch === epoch);
    if (!row || !isEpochRowDirty(row) || row.pending) return;

    const normalizedName = normalizeEpochName(row.selected_name);

    updateEpochRaceRow(epoch, (current) => ({
      ...current,
      pending: true,
      status: "saved",
    }));

    try {
      const pendingSaves: Promise<unknown>[] = [];
      if (row.selected_race_id !== row.saved_race_id) {
        pendingSaves.push(
          api.setStreamEpochRace(streamId, epoch, row.selected_race_id),
        );
      }
      if (normalizedName !== row.saved_name) {
        pendingSaves.push(
          api.setStreamEpochName(streamId, epoch, normalizedName),
        );
      }
      await Promise.all(pendingSaves);
      updateEpochRaceRow(epoch, (current) => ({
        ...current,
        saved_race_id: current.selected_race_id,
        selected_name: normalizeEpochName(current.selected_name) ?? "",
        saved_name: normalizeEpochName(current.selected_name),
        pending: false,
        status: "saved",
      }));
    } catch {
      updateEpochRaceRow(epoch, (current) => ({
        ...current,
        pending: false,
        status: "error",
      }));
    }
  }

  function getCurrentEpochRow(): EpochRaceRow | null {
    return (
      epochRaceRows.find((row) => row.is_current) ??
      epochRaceRows.find((row) => row.epoch === stream?.stream_epoch) ??
      null
    );
  }

  function canAdvanceToNextEpoch(): boolean {
    const row = getCurrentEpochRow();
    if (!row) return false;
    if (!row.saved_race_id) return false;
    if (isEpochRowDirty(row)) return false;
    if (row.pending) return false;
    if (epochAdvancePending) return false;
    return true;
  }

  async function handleAdvanceToNextEpoch(): Promise<void> {
    const row = getCurrentEpochRow();
    const raceId = row?.saved_race_id ?? null;
    if (!raceId || !canAdvanceToNextEpoch()) return;
    epochAdvancePending = true;
    epochAdvanceStatus = "idle";
    try {
      await api.activateNextStreamEpochForRace(raceId, streamId);
      epochAdvanceAwaitingReload = true;
    } catch {
      epochAdvanceStatus = "error";
      epochAdvancePending = false;
    }
  }

  function epochRowStatusText(row: EpochRaceRow): string {
    if (row.pending) return "Saving...";
    if (row.status === "error") return "Error";
    if (isEpochRowDirty(row)) return "Unsaved";
    if (row.status === "incomplete") return "Unverified";
    return "Saved";
  }

  // Lightweight refresh: fetch epoch data and merge only event_count into
  // existing rows, preserving any pending or dirty edits.
  async function refreshEpochEventCounts(id: string): Promise<void> {
    if (epochRaceRows.length === 0) return;
    try {
      const freshEpochs = await api.getStreamEpochs(id);
      const countByEpoch = new Map(
        freshEpochs.map((e) => [e.epoch, e.event_count]),
      );
      epochRaceRows = epochRaceRows.map((row) => {
        if (row.pending || isEpochRowDirty(row)) return row;
        const freshCount = countByEpoch.get(row.epoch);
        if (freshCount !== undefined && freshCount !== row.event_count) {
          return { ...row, event_count: freshCount };
        }
        return row;
      });
    } catch {
      // Silently ignore — the full loadEpochRaceRows will surface errors.
    }
  }

  // Poll event counts every 5 seconds while the page is visible
  $effect(() => {
    // Capture the current streamId so the effect re-subscribes on navigation
    const id = streamId;
    if (!id) return;

    let intervalHandle: ReturnType<typeof setInterval> | null = null;

    function startPolling(): void {
      if (intervalHandle) return;
      intervalHandle = setInterval(() => {
        void refreshEpochEventCounts(id);
      }, 5000);
    }

    function stopPolling(): void {
      if (intervalHandle) {
        clearInterval(intervalHandle);
        intervalHandle = null;
      }
    }

    function onVisibilityChange(): void {
      if (document.hidden) {
        stopPolling();
      } else {
        startPolling();
      }
    }

    // Start immediately if visible
    if (!document.hidden) {
      startPolling();
    }

    document.addEventListener("visibilitychange", onVisibilityChange);

    return () => {
      stopPolling();
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  });
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
      <div class="ml-auto">
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
      </div>
    {/if}
  </div>

  {#if stream}
    <div class="grid grid-cols-2 gap-4 mb-6 items-start">
      <Card title="Info">
        <dl
          class="grid gap-y-2 gap-x-4 text-sm m-0"
          style="grid-template-columns: auto 1fr;"
        >
          <dt class="text-text-muted">Stream ID</dt>
          <dd class="font-mono text-text-primary m-0">{stream.stream_id}</dd>
          <dt class="text-text-muted">Forwarder</dt>
          <dd class="text-text-primary m-0">{stream.forwarder_id}</dd>
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

    <div class="mb-6">
      <Card title="Epoch Race Mapping">
        {#if epochRaceRowsLoading}
          <p class="text-sm text-text-muted italic m-0">Loading epochs...</p>
        {:else if epochRaceRowsError}
          <p class="text-sm text-status-err m-0">
            Failed to load epoch mappings.
          </p>
        {:else if epochRaceRows.length === 0}
          <p class="text-sm text-text-muted m-0">
            No epochs available for mapping.
          </p>
        {:else}
          {#if epochRaceRowsHydrationIncomplete}
            <p class="text-sm text-status-err mb-3 m-0">
              Some race mappings could not be loaded. Epoch status is unverified
              until reloaded or explicitly saved.
            </p>
          {/if}
          <div class="overflow-x-auto rounded-md border border-border">
            <table class="w-full text-sm border-collapse">
              <thead class="bg-surface-1">
                <tr>
                  <th
                    class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider border-b border-border"
                  >
                    Epoch
                  </th>
                  <th
                    class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider border-b border-border"
                  >
                    Events
                  </th>
                  <th
                    class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider border-b border-border"
                  >
                    Race
                  </th>
                  <th
                    class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider border-b border-border"
                  >
                    Name
                  </th>
                  <th
                    class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider border-b border-border"
                  >
                    Save
                  </th>
                  <th
                    class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider border-b border-border"
                  >
                    Status
                  </th>
                  <th
                    class="px-3 py-2 text-left text-xs font-medium text-text-muted uppercase tracking-wider border-b border-border"
                  >
                    Export
                  </th>
                </tr>
              </thead>
              <tbody>
                {#each epochRaceRows as row (row.epoch)}
                  <tr class="border-b border-border">
                    <td class="px-3 py-2 font-mono">
                      {row.epoch}{row.is_current ? " (current)" : ""}
                    </td>
                    <td class="px-3 py-2 font-mono">
                      {row.event_count.toLocaleString()}
                    </td>
                    <td class="px-3 py-2">
                      <select
                        data-testid={`epoch-race-select-${row.epoch}`}
                        value={row.selected_race_id ?? ""}
                        disabled={row.pending}
                        onchange={(e) => {
                          const value = e.currentTarget.value;
                          onEpochRaceSelectChange(row.epoch, value || null);
                        }}
                        class="text-xs px-2 py-1 rounded-md border border-border bg-surface-0 text-text-primary disabled:opacity-50"
                      >
                        <option value="">No race</option>
                        {#each $racesStore as race (race.race_id)}
                          <option value={race.race_id}>{race.name}</option>
                        {/each}
                      </select>
                    </td>
                    <td class="px-3 py-2">
                      <input
                        data-testid={`epoch-name-input-${row.epoch}`}
                        type="text"
                        value={row.selected_name}
                        disabled={row.pending}
                        oninput={(e) => {
                          onEpochNameInput(row.epoch, e.currentTarget.value);
                        }}
                        class="w-full min-w-40 text-xs px-2 py-1 rounded-md border border-border bg-surface-0 text-text-primary disabled:opacity-50"
                      />
                    </td>
                    <td class="px-3 py-2">
                      <button
                        data-testid={`epoch-race-save-${row.epoch}`}
                        onclick={() => void handleSaveEpochRace(row.epoch)}
                        disabled={!isEpochRowDirty(row) || row.pending}
                        class="px-3 py-1 text-xs font-medium rounded-md bg-status-ok-bg border border-status-ok-border text-status-ok cursor-pointer hover:opacity-90 disabled:opacity-50 disabled:cursor-not-allowed"
                      >
                        {row.pending ? "Saving..." : "Save"}
                      </button>
                    </td>
                    <td class="px-3 py-2">
                      <span
                        data-testid={`epoch-race-state-${row.epoch}`}
                        class="text-xs {epochRowStatusText(row) === 'Error'
                          ? 'text-status-err'
                          : epochRowStatusText(row) === 'Saved'
                            ? 'text-status-ok'
                            : 'text-text-muted'}"
                      >
                        {epochRowStatusText(row)}
                      </span>
                    </td>
                    <td class="px-3 py-2">
                      <a
                        data-testid={`epoch-export-csv-${row.epoch}`}
                        href={api.epochExportCsvUrl(streamId, row.epoch)}
                        download
                        class="text-xs text-accent no-underline hover:underline"
                      >
                        CSV
                      </a>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
          <div class="mt-3 flex items-center gap-3">
            <button
              data-testid="epoch-race-advance-next-btn"
              onclick={() => void handleAdvanceToNextEpoch()}
              disabled={!canAdvanceToNextEpoch()}
              class="px-3 py-1 text-xs font-medium rounded-md bg-surface-2 border border-border text-text-secondary cursor-pointer hover:bg-surface-3 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {epochAdvancePending && epochAdvanceAwaitingReload
                ? "Reloading..."
                : epochAdvancePending
                  ? "Advancing..."
                  : epochAdvanceStatus === "error"
                    ? "Advance failed"
                    : "Advance to Next Epoch"}
            </button>
            <span class="text-xs text-text-muted">
              Uses the current epoch's saved race mapping.
            </span>
          </div>
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

    <div class="mb-6">
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
    </div>
  {:else}
    <p class="text-sm text-text-muted">
      Stream not found. It may appear once the forwarder connects.
    </p>
  {/if}
</main>
