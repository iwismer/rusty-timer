<script lang="ts">
  import { onDestroy } from "svelte";
  import * as api from "$lib/api";
  import {
    streamsStore,
    metricsStore,
    setMetrics,
    forwarderRacesStore,
    racesStore,
    racesLoadedStore,
    setForwarderRace,
  } from "$lib/stores";
  import { shouldFetchMetrics } from "$lib/streamMetricsLoader";
  import { groupStreamsByForwarder } from "$lib/groupStreams";
  import {
    readHideOfflinePreference,
    writeHideOfflinePreference,
  } from "$lib/hideOfflinePreference";
  import {
    readRaceFilterPreference,
    writeRaceFilterPreference,
  } from "$lib/raceFilterPreference";
  import { StatusBadge, Card, AlertBanner } from "@rusty-timer/shared-ui";
  import {
    formatReadMode,
    formatTtoState,
    formatClockDrift,
    readerControlDisabled,
    computeDownloadPercent,
  } from "@rusty-timer/shared-ui/lib/reader-view-model";
  import {
    READ_MODE_OPTIONS,
    shouldShowTimeoutInput,
    initialTimeoutDraft,
    resolveTimeoutSeconds,
  } from "@rusty-timer/shared-ui/lib/read-mode-form";
  import { readerStatesStore, downloadProgressStore } from "$lib/stores";
  import {
    syncReaderClock,
    setReaderReadMode,
    setReaderTto,
    setReaderRecording,
    clearReaderRecords,
    startReaderDownload,
    refreshReader,
    reconnectReader,
  } from "$lib/api";
  import { resolveChipRead } from "$lib/chipResolver";
  import { raceDataStore, ensureRaceDataLoaded } from "$lib/raceDataLoader";

  // Reader control state
  let expandedReader = $state<string | null>(null);
  let controlBusy: Record<string, boolean> = $state({});
  let controlFeedback: Record<
    string,
    { kind: "ok" | "err"; message: string } | undefined
  > = $state({});
  let readModeDrafts: Record<string, string> = $state({});
  let readModeTimeoutDrafts: Record<string, string> = $state({});

  function toggleReaderExpand(key: string) {
    expandedReader = expandedReader === key ? null : key;
  }

  function readerKey(forwarderId: string, readerIp: string): string {
    return `${forwarderId}:${readerIp}`;
  }

  function readModeDraftValue(
    key: string,
    info: api.ReaderInfo | null | undefined,
  ): "raw" | "event" | "fsls" {
    return ((readModeDrafts[key] as "raw" | "event" | "fsls" | undefined) ??
      info?.config?.mode ??
      "raw") as "raw" | "event" | "fsls";
  }

  function readModeTimeoutDraftValue(
    key: string,
    info: api.ReaderInfo | null | undefined,
  ): string {
    return (
      readModeTimeoutDrafts[key] ?? initialTimeoutDraft(info?.config?.timeout)
    );
  }

  function updateReadModeDraft(
    key: string,
    mode: "raw" | "event" | "fsls",
    info: api.ReaderInfo | null | undefined,
  ) {
    readModeDrafts = { ...readModeDrafts, [key]: mode };
    if (shouldShowTimeoutInput(mode) && readModeTimeoutDrafts[key] == null) {
      readModeTimeoutDrafts = {
        ...readModeTimeoutDrafts,
        [key]: initialTimeoutDraft(info?.config?.timeout),
      };
    }
  }

  function updateReadModeTimeoutDraft(key: string, value: string) {
    readModeTimeoutDrafts = { ...readModeTimeoutDrafts, [key]: value };
  }

  async function handleSyncClock(
    forwarderId: string,
    readerIp: string,
    key: string,
  ) {
    controlBusy[key] = true;
    controlFeedback[key] = undefined;
    try {
      const resp = await syncReaderClock(forwarderId, readerIp);
      controlFeedback[key] = {
        kind: "ok",
        message: `Clock synced — drift: ${formatClockDrift(resp.reader_info?.clock?.drift_ms)}`,
      };
    } catch (e: any) {
      controlFeedback[key] = {
        kind: "err",
        message: e.message ?? "Failed to sync clock",
      };
    }
    controlBusy[key] = false;
  }

  async function handleSetReadMode(
    forwarderId: string,
    readerIp: string,
    key: string,
    mode: "raw" | "event" | "fsls",
    timeoutDraft: string,
    currentTimeout: number | null | undefined,
  ) {
    const timeout = resolveTimeoutSeconds(timeoutDraft, currentTimeout);
    controlBusy[key] = true;
    controlFeedback[key] = undefined;
    try {
      const resp = await setReaderReadMode(
        forwarderId,
        readerIp,
        mode,
        timeout,
      );
      readModeDrafts = { ...readModeDrafts, [key]: mode };
      readModeTimeoutDrafts = {
        ...readModeTimeoutDrafts,
        [key]: String(timeout),
      };
      controlFeedback[key] = {
        kind: "ok",
        message: shouldShowTimeoutInput(mode)
          ? `Mode set to ${formatReadMode(mode)} (${timeout}s)`
          : `Mode set to ${formatReadMode(mode)}`,
      };
    } catch (e: any) {
      controlFeedback[key] = {
        kind: "err",
        message: e.message ?? "Set mode failed",
      };
    }
    controlBusy[key] = false;
  }

  async function handleSetTto(
    forwarderId: string,
    readerIp: string,
    key: string,
    info: api.ReaderInfo | null | undefined,
  ) {
    const currentlyEnabled = info?.tto_enabled === true;
    controlBusy[key] = true;
    controlFeedback[key] = undefined;
    try {
      await setReaderTto(forwarderId, readerIp, !currentlyEnabled);
      controlFeedback[key] = {
        kind: "ok",
        message: currentlyEnabled
          ? "TTO reporting disabled"
          : "TTO reporting enabled",
      };
    } catch (e: any) {
      controlFeedback[key] = {
        kind: "err",
        message: e.message ?? "TTO toggle failed",
      };
    }
    controlBusy[key] = false;
  }

  async function handleSetRecording(
    forwarderId: string,
    readerIp: string,
    key: string,
    info: api.ReaderInfo | null | undefined,
  ) {
    const currentlyRecording = info?.recording === true;
    controlBusy[key] = true;
    controlFeedback[key] = undefined;
    try {
      await setReaderRecording(forwarderId, readerIp, !currentlyRecording);
      controlFeedback[key] = {
        kind: "ok",
        message: currentlyRecording ? "Recording stopped" : "Recording started",
      };
    } catch (e: any) {
      controlFeedback[key] = {
        kind: "err",
        message: e.message ?? "Toggle recording failed",
      };
    }
    controlBusy[key] = false;
  }

  async function handleRefresh(
    forwarderId: string,
    readerIp: string,
    key: string,
  ) {
    controlBusy[key] = true;
    controlFeedback[key] = undefined;
    try {
      await refreshReader(forwarderId, readerIp);
      controlFeedback[key] = { kind: "ok", message: "Reader info refreshed" };
    } catch (e: any) {
      controlFeedback[key] = {
        kind: "err",
        message: e.message ?? "Refresh failed",
      };
    }
    controlBusy[key] = false;
  }

  async function handleClearRecords(
    forwarderId: string,
    readerIp: string,
    key: string,
  ) {
    controlBusy[key] = true;
    controlFeedback[key] = undefined;
    try {
      await clearReaderRecords(forwarderId, readerIp);
      controlFeedback[key] = { kind: "ok", message: "Records cleared" };
    } catch (e: any) {
      controlFeedback[key] = {
        kind: "err",
        message: e.message ?? "Clear failed",
      };
    }
    controlBusy[key] = false;
  }

  async function handleStartDownload(
    forwarderId: string,
    readerIp: string,
    key: string,
  ) {
    controlBusy[key] = true;
    controlFeedback[key] = undefined;
    try {
      await startReaderDownload(forwarderId, readerIp);
      controlFeedback[key] = { kind: "ok", message: "Download started" };
    } catch (e: any) {
      controlFeedback[key] = {
        kind: "err",
        message: e.message ?? "Download failed",
      };
    }
    controlBusy[key] = false;
  }

  async function handleReconnect(
    forwarderId: string,
    readerIp: string,
    key: string,
  ) {
    controlBusy[key] = true;
    controlFeedback[key] = undefined;
    try {
      await reconnectReader(forwarderId, readerIp);
      controlFeedback[key] = { kind: "ok", message: "Reconnect requested" };
    } catch (e: any) {
      controlFeedback[key] = {
        kind: "err",
        message: e.message ?? "Reconnect failed",
      };
    }
    controlBusy[key] = false;
  }

  // Metrics fetching state
  let requestedMetricStreamIds = $state(new Set<string>());
  let inFlightMetricStreamIds = $state(new Set<string>());
  const METRICS_RETRY_DELAY_MS = 1000;
  let metricsRetryTimers: Record<string, ReturnType<typeof setTimeout>> = {};

  // Fetch initial metrics for all streams
  $effect(() => {
    for (const s of $streamsStore) {
      void maybeFetchMetrics(s.stream_id);
    }
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
      const nextRequested = new Set(requestedMetricStreamIds);
      nextRequested.delete(id);
      requestedMetricStreamIds = nextRequested;
      scheduleMetricsRetry(id);
    } finally {
      const next = new Set(inFlightMetricStreamIds);
      next.delete(id);
      inFlightMetricStreamIds = next;
    }
  }

  function scheduleMetricsRetry(id: string): void {
    if (metricsRetryTimers[id]) return;
    metricsRetryTimers[id] = setTimeout(() => {
      delete metricsRetryTimers[id];
      void maybeFetchMetrics(id);
    }, METRICS_RETRY_DELAY_MS);
  }

  // Group streams by forwarder_id.
  let groupedStreams = $derived(groupStreamsByForwarder($streamsStore));
  let groupedStreamsById = $derived(
    new Map(groupedStreams.map((g) => [g.forwarderId, g])),
  );

  // Hide-offline toggle (persisted to localStorage)
  let hideOffline = $state(readHideOfflinePreference());
  $effect(() => {
    writeHideOfflinePreference(hideOffline);
  });

  // Race filter (persisted to localStorage)
  let selectedRaceId = $state<string | null>(readRaceFilterPreference());

  $effect(() => {
    writeRaceFilterPreference(selectedRaceId);
  });

  // Reset if the persisted race no longer exists (only after races load)
  $effect(() => {
    if (
      $racesLoadedStore &&
      selectedRaceId &&
      !$racesStore.some((r) => r.race_id === selectedRaceId)
    ) {
      selectedRaceId = null;
    }
  });

  // Fail-open behavior: if races are unavailable or the selected race is missing,
  // treat it as "All races" instead of filtering everything out.
  let effectiveSelectedRaceId = $derived(
    selectedRaceId &&
      $racesStore.some((race) => race.race_id === selectedRaceId)
      ? selectedRaceId
      : null,
  );

  let visibleGroups = $derived.by(() => {
    let groups = groupedStreams;
    if (effectiveSelectedRaceId) {
      groups = groups.filter((g) => {
        const forwarderRaceId = $forwarderRacesStore[g.forwarderId];
        return forwarderRaceId === effectiveSelectedRaceId;
      });
    }
    if (hideOffline) {
      groups = groups
        .map((g) => ({
          ...g,
          streams: g.streams.filter((s) => s.online),
        }))
        .filter((g) => g.streams.length > 0);
    }
    return groups;
  });

  // Time-since-last-read helpers
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

  function timeSinceLastRead(streamId: string): string {
    const m = $metricsStore[streamId];
    if (!m?.epoch_last_received_at) return "\u2014";
    const elapsed = Date.now() - new Date(m.epoch_last_received_at).getTime();
    return formatDuration(Math.max(0, elapsed));
  }

  // Tick to force re-render every second for time-since-last-read
  let tick = $state(0);
  const timerHandle = setInterval(() => {
    tick++;
  }, 1000);
  onDestroy(() => {
    clearInterval(timerHandle);
    for (const timer of Object.values(metricsRetryTimers)) {
      clearTimeout(timer);
    }
    metricsRetryTimers = {};
  });

  // Aggregate stats per forwarder group.
  // metricsMap is passed explicitly so Svelte tracks it as a reactive dependency
  // in the {@const} call site — without it, the group header stats won't live-update.
  function groupStats(
    streams: typeof $streamsStore,
    metricsMap: Record<string, import("$lib/api").StreamMetrics>,
  ) {
    let totalRaw = 0;
    let totalChips = 0;
    let onlineCount = 0;
    for (const s of streams) {
      const m = metricsMap[s.stream_id];
      if (m) {
        totalRaw += m.epoch_raw_count;
        totalChips += m.unique_chips;
      }
      if (s.online) onlineCount++;
    }
    return { totalRaw, totalChips, onlineCount, totalStreams: streams.length };
  }

  function groupBorderStatus(
    stats: ReturnType<typeof groupStats>,
  ): "ok" | "warn" | "err" | undefined {
    if (stats.totalStreams === 0) return undefined;
    if (stats.onlineCount === 0) return "err";
    if (stats.onlineCount < stats.totalStreams) return "warn";
    return undefined;
  }

  // Load race data whenever forwarder-race assignments change
  $effect(() => {
    for (const raceId of Object.values($forwarderRacesStore)) {
      if (raceId) void ensureRaceDataLoaded(raceId);
    }
  });

  async function handleRaceChange(forwarderId: string, raceId: string | null) {
    const previousRaceId = $forwarderRacesStore[forwarderId] ?? null;
    setForwarderRace(forwarderId, raceId);
    try {
      await api.setForwarderRace(forwarderId, raceId);
    } catch {
      setForwarderRace(forwarderId, previousRaceId);
    }
  }

  function lastReadDisplay(streamId: string): string {
    const m = $metricsStore[streamId];
    if (!m) return "\u2014";
    if (!m.last_tag_id && !m.last_reader_timestamp) return "\u2014";
    // Find the forwarder for this stream to get its race assignment
    const stream = $streamsStore.find((s) => s.stream_id === streamId);
    if (!stream) return "\u2014";
    const raceId = $forwarderRacesStore[stream.forwarder_id];
    const raceData = raceId ? $raceDataStore[raceId] : null;
    return resolveChipRead(
      m.last_tag_id,
      m.last_reader_timestamp,
      raceData?.chipMap ?? null,
    );
  }
</script>

<main class="max-w-[1100px] mx-auto px-6 py-6">
  <div class="flex items-center justify-between mb-6">
    <h1
      data-testid="streams-heading"
      class="text-xl font-bold text-text-primary m-0"
    >
      Streams
    </h1>
    <div class="flex items-center gap-4">
      <select
        data-testid="race-filter-select"
        aria-label="Filter streams by race"
        class="text-sm px-2 py-1 rounded-md border border-border bg-surface-0 text-text-primary"
        value={effectiveSelectedRaceId ?? ""}
        onchange={(e) => {
          selectedRaceId = e.currentTarget.value || null;
        }}
      >
        <option value="">All races</option>
        {#each $racesStore as race (race.race_id)}
          <option value={race.race_id}>{race.name}</option>
        {/each}
      </select>
      <label
        class="flex items-center gap-2 text-sm text-text-muted cursor-pointer select-none"
      >
        <input
          type="checkbox"
          bind:checked={hideOffline}
          class="cursor-pointer"
        />
        Hide offline
      </label>
    </div>
  </div>

  {#each visibleGroups as group (group.forwarderId)}
    {@const fullGroup = groupedStreamsById.get(group.forwarderId)}
    {@const stats = groupStats(
      fullGroup?.streams ?? group.streams,
      $metricsStore,
    )}
    {@const border = groupBorderStatus(stats)}
    <div class="mb-6">
      <Card borderStatus={border} headerBg>
        {#snippet header()}
          <h2 class="text-sm font-semibold text-text-primary m-0">
            {group.displayName}
          </h2>
          <StatusBadge
            label="{stats.onlineCount}/{stats.totalStreams} online"
            state={stats.onlineCount === 0
              ? "err"
              : stats.onlineCount < stats.totalStreams
                ? "warn"
                : "ok"}
          />
          <div class="ml-auto flex items-center gap-3">
            <span class="text-xs text-text-muted">
              {stats.totalRaw.toLocaleString()} reads &middot;
              {stats.totalChips.toLocaleString()} chips
            </span>
            <select
              data-testid={`forwarder-race-select-${group.forwarderId}`}
              aria-label={`Assign race for ${group.displayName}`}
              class="text-xs px-2 py-1 rounded-md border border-border bg-surface-0 text-text-primary"
              value={$forwarderRacesStore[group.forwarderId] ?? ""}
              onchange={(e) => {
                const val = e.currentTarget.value;
                handleRaceChange(group.forwarderId, val || null);
              }}
            >
              <option value="">No race</option>
              {#each $racesStore as race (race.race_id)}
                <option value={race.race_id}>{race.name}</option>
              {/each}
            </select>
            <a
              href="/forwarders/{group.forwarderId}/reads"
              class="text-xs font-medium px-2.5 py-1 rounded-md text-accent no-underline bg-accent-bg hover:underline"
            >
              View Reads
            </a>
            <a
              href="/forwarders/{group.forwarderId}/config"
              class="text-xs font-medium px-2.5 py-1 rounded-md text-accent no-underline bg-accent-bg hover:underline"
            >
              Configure
            </a>
          </div>
        {/snippet}

        <div data-testid="stream-list" class="grid gap-3">
          {#each group.streams as stream (stream.stream_id)}
            <div
              data-testid="stream-item"
              class="rounded-md p-4 bg-surface-0 border {stream.online
                ? 'border-border'
                : 'border-status-err-border'}"
            >
              <div class="flex items-center gap-2 mb-3">
                <a
                  data-testid="stream-detail-link"
                  href="/streams/{stream.stream_id}"
                  class="text-sm font-semibold text-accent no-underline hover:underline"
                >
                  {stream.display_alias || stream.reader_ip}
                </a>
                {#if stream.online}
                  <span data-testid="stream-online-badge">
                    <StatusBadge label="online" state="ok" />
                  </span>
                {:else}
                  <span data-testid="stream-offline-badge">
                    <StatusBadge label="offline" state="err" />
                  </span>
                {/if}
                <button
                  onclick={() =>
                    toggleReaderExpand(
                      readerKey(stream.forwarder_id, stream.reader_ip),
                    )}
                  class="ml-auto text-xs text-text-muted hover:text-text-primary transition-colors flex items-center gap-1"
                  aria-expanded={expandedReader ===
                    readerKey(stream.forwarder_id, stream.reader_ip)}
                >
                  Details
                  <span
                    class="inline-block transition-transform {expandedReader ===
                    readerKey(stream.forwarder_id, stream.reader_ip)
                      ? 'rotate-180'
                      : ''}">▾</span
                  >
                </button>
              </div>

              <div class="flex gap-6 mb-3">
                {#if $metricsStore[stream.stream_id]}
                  <div class="shrink-0">
                    <p class="text-xs text-text-muted m-0">Reads</p>
                    <p
                      class="text-lg font-bold font-mono text-text-primary m-0"
                    >
                      {$metricsStore[
                        stream.stream_id
                      ].epoch_raw_count.toLocaleString()}
                    </p>
                  </div>
                  <div class="shrink-0">
                    <p class="text-xs text-text-muted m-0">Chips</p>
                    <p
                      class="text-lg font-bold font-mono text-text-primary m-0"
                    >
                      {$metricsStore[
                        stream.stream_id
                      ].unique_chips.toLocaleString()}
                    </p>
                  </div>
                  <div class="min-w-0 flex-1">
                    <p class="text-xs text-text-muted m-0">Last read</p>
                    <p
                      class="text-sm font-mono text-text-primary m-0 truncate"
                      title={lastReadDisplay(stream.stream_id)}
                    >
                      {lastReadDisplay(stream.stream_id)}
                    </p>
                    <p class="text-xs text-text-muted m-0">
                      {tick !== undefined
                        ? timeSinceLastRead(stream.stream_id)
                        : ""}
                    </p>
                  </div>
                {:else}
                  <div>
                    <p class="text-sm text-text-muted italic m-0">
                      Loading metrics…
                    </p>
                  </div>
                {/if}
              </div>

              <div class="flex items-center gap-3 text-xs text-text-muted">
                <span class="font-mono">{stream.reader_ip}</span>
                <span>&middot;</span>
                <span>epoch {stream.stream_epoch}</span>
              </div>

              {#if expandedReader === readerKey(stream.forwarder_id, stream.reader_ip)}
                {@const key = readerKey(stream.forwarder_id, stream.reader_ip)}
                {@const rs = $readerStatesStore[key]}
                {@const info = rs?.reader_info}
                {@const busy = controlBusy[key]}
                {@const disabled =
                  !stream.online ||
                  readerControlDisabled(rs?.state ?? "disconnected", busy)}
                {@const dp = $downloadProgressStore[key]}

                <div class="mt-4 pt-4 border-t border-border">
                  {#if !rs}
                    <p class="text-sm text-text-muted">
                      No reader data available
                    </p>
                  {:else}
                    <!-- Info grid -->
                    <div class="grid grid-cols-2 gap-x-8 gap-y-2 text-sm mb-4">
                      {#if info?.banner}
                        <div class="col-span-2">
                          <span class="text-text-muted">Banner:</span>
                          <span class="font-mono ml-2 text-xs"
                            >{info.banner}</span
                          >
                        </div>
                      {/if}
                      <div>
                        <span class="text-text-muted">Firmware:</span>
                        <span class="font-mono ml-2"
                          >{info?.hardware?.fw_version ?? "\u2014"}</span
                        >
                      </div>
                      <div>
                        <span class="text-text-muted">Hardware:</span>
                        <span class="font-mono ml-2"
                          >{info?.hardware?.hw_code ?? "\u2014"}</span
                        >
                      </div>
                      <div>
                        <span class="text-text-muted">Clock Drift:</span>
                        <span class="font-mono ml-2"
                          >{formatClockDrift(info?.clock?.drift_ms)}</span
                        >
                      </div>
                      <div>
                        <span class="text-text-muted">Reader State:</span>
                        <span class="ml-2">
                          <StatusBadge
                            label={rs.state}
                            state={rs.state === "connected"
                              ? "ok"
                              : rs.state === "connecting"
                                ? "warn"
                                : "err"}
                          />
                        </span>
                      </div>
                      <div>
                        <span class="text-text-muted">Read Mode:</span>
                        <span class="font-mono ml-2"
                          >{formatReadMode(info?.config?.mode)}</span
                        >
                      </div>
                      <div>
                        <span class="text-text-muted">TTO:</span>
                        <span class="font-mono ml-2"
                          >{formatTtoState(info?.tto_enabled)}</span
                        >
                      </div>
                      <div>
                        <span class="text-text-muted">Recording:</span>
                        <span class="font-mono ml-2"
                          >{info?.recording == null
                            ? "\u2014"
                            : info.recording
                              ? "Yes"
                              : "No"}</span
                        >
                      </div>
                      <div>
                        <span class="text-text-muted">Stored Reads:</span>
                        <span class="font-mono ml-2"
                          >{info?.estimated_stored_reads?.toLocaleString() ??
                            "\u2014"}</span
                        >
                      </div>
                    </div>

                    <!-- Read mode controls -->
                    <div class="col-span-2 mb-4">
                      <span class="text-sm text-text-muted">Read Mode:</span>
                      <span
                        class="ml-2 inline-flex items-center gap-2 flex-wrap"
                      >
                        <select
                          class="px-2 py-0.5 text-sm rounded-md bg-surface-0 text-text-primary border border-border"
                          value={readModeDraftValue(key, info)}
                          onchange={(e) => {
                            updateReadModeDraft(
                              key,
                              (e.currentTarget as HTMLSelectElement).value as
                                | "raw"
                                | "event"
                                | "fsls",
                              info,
                            );
                          }}
                          {disabled}
                        >
                          {#each READ_MODE_OPTIONS as option}
                            <option value={option.value}>{option.label}</option>
                          {/each}
                        </select>
                        {#if shouldShowTimeoutInput(readModeDraftValue(key, info))}
                          <label
                            class="inline-flex items-center gap-1 text-xs text-text-muted"
                          >
                            <span>Timeout</span>
                            <input
                              class="w-16 px-2 py-0.5 text-sm rounded-md bg-surface-0 text-text-primary border border-border"
                              type="number"
                              min="1"
                              max="255"
                              value={readModeTimeoutDraftValue(key, info)}
                              oninput={(e) => {
                                updateReadModeTimeoutDraft(
                                  key,
                                  (e.currentTarget as HTMLInputElement).value,
                                );
                              }}
                              {disabled}
                            />
                            <span>s</span>
                          </label>
                        {/if}
                        <button
                          class="px-2.5 py-0.5 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
                          onclick={() => {
                            handleSetReadMode(
                              stream.forwarder_id,
                              stream.reader_ip,
                              key,
                              readModeDraftValue(key, info),
                              readModeTimeoutDraftValue(key, info),
                              info?.config?.timeout,
                            );
                          }}
                          {disabled}>Apply</button
                        >
                      </span>
                    </div>

                    <!-- TTO toggle -->
                    <div class="mb-4">
                      <span class="text-sm text-text-muted">TTO Bytes:</span>
                      <span
                        class="ml-2 inline-flex items-center gap-2 flex-wrap"
                      >
                        <span class="font-mono text-sm"
                          >{formatTtoState(info?.tto_enabled)}</span
                        >
                        <button
                          class="px-2.5 py-0.5 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
                          onclick={() =>
                            handleSetTto(
                              stream.forwarder_id,
                              stream.reader_ip,
                              key,
                              info,
                            )}
                          {disabled}
                        >
                          {info?.tto_enabled ? "Disable TTO" : "Enable TTO"}
                        </button>
                      </span>
                    </div>

                    <!-- Action buttons row -->
                    <div
                      class="flex items-center gap-3 pt-3 border-t border-border flex-wrap"
                    >
                      <button
                        class="px-3 py-1.5 text-sm font-medium rounded-md text-white bg-accent border-none cursor-pointer hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed"
                        onclick={() =>
                          handleSyncClock(
                            stream.forwarder_id,
                            stream.reader_ip,
                            key,
                          )}
                        {disabled}>Sync Clock</button
                      >
                      <button
                        class="px-3 py-1.5 text-sm rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
                        onclick={() =>
                          handleRefresh(
                            stream.forwarder_id,
                            stream.reader_ip,
                            key,
                          )}
                        {disabled}>Refresh</button
                      >
                      <button
                        class={info?.recording
                          ? "px-3 py-1.5 text-sm rounded-md bg-red-600 text-white border-none cursor-pointer hover:bg-red-700 disabled:opacity-50"
                          : "px-3 py-1.5 text-sm rounded-md bg-green-600 text-white border-none cursor-pointer hover:bg-green-700 disabled:opacity-50"}
                        onclick={() =>
                          handleSetRecording(
                            stream.forwarder_id,
                            stream.reader_ip,
                            key,
                            info,
                          )}
                        {disabled}
                        >{info?.recording
                          ? "Stop Recording"
                          : "Start Recording"}</button
                      >
                      <button
                        class="px-3 py-1.5 text-sm font-medium rounded-md text-white bg-accent border-none cursor-pointer hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed"
                        onclick={() =>
                          handleStartDownload(
                            stream.forwarder_id,
                            stream.reader_ip,
                            key,
                          )}
                        {disabled}>Download Reads</button
                      >
                      <button
                        class="px-3 py-1.5 text-sm rounded-md bg-red-600 text-white border-none cursor-pointer hover:bg-red-700 disabled:opacity-50"
                        onclick={() =>
                          handleClearRecords(
                            stream.forwarder_id,
                            stream.reader_ip,
                            key,
                          )}
                        {disabled}>Clear Records</button
                      >
                      <button
                        class="px-3 py-1.5 text-sm rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
                        onclick={() =>
                          handleReconnect(
                            stream.forwarder_id,
                            stream.reader_ip,
                            key,
                          )}
                        disabled={busy}>Reconnect</button
                      >
                    </div>

                    <!-- Download progress bar -->
                    {#if dp?.state === "downloading"}
                      {@const percent = computeDownloadPercent(
                        dp,
                        info?.estimated_stored_reads,
                      )}
                      <div
                        class="mt-3 flex items-center gap-3 text-sm text-text-secondary"
                      >
                        <div
                          class="flex-1 h-2 rounded-full bg-surface-2 overflow-hidden"
                        >
                          <div
                            class="h-full bg-accent rounded-full transition-all"
                            style="width: {percent}%"
                          ></div>
                        </div>
                        <span class="text-xs font-mono whitespace-nowrap">
                          {dp.reads_received} reads &middot; {percent}%
                        </span>
                      </div>
                    {/if}

                    <!-- Feedback banner -->
                    {#if controlFeedback[key]}
                      {@const fb = controlFeedback[key]}
                      {#if fb}
                        <div class="mt-3">
                          <AlertBanner
                            variant={fb.kind}
                            message={fb.message}
                            onDismiss={() => {
                              controlFeedback = {
                                ...controlFeedback,
                                [key]: undefined,
                              };
                            }}
                          />
                        </div>
                      {/if}
                    {/if}
                  {/if}
                </div>
              {/if}
            </div>
          {/each}
        </div>
      </Card>
    </div>
  {/each}

  {#if visibleGroups.length === 0}
    {#if $streamsStore.length === 0}
      <p class="text-sm text-text-muted">No streams found.</p>
    {:else if effectiveSelectedRaceId && hideOffline}
      <p class="text-sm text-text-muted">
        No online streams match the selected race.
      </p>
    {:else if effectiveSelectedRaceId}
      <p class="text-sm text-text-muted">No streams match the selected race.</p>
    {:else if hideOffline}
      <p data-testid="no-online-streams" class="text-sm text-text-muted">
        No online streams found.
      </p>
    {/if}
  {/if}
</main>
