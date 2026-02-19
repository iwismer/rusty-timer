<script lang="ts">
  import { onDestroy } from "svelte";
  import * as api from "$lib/api";
  import { streamsStore, metricsStore, setMetrics } from "$lib/stores";
  import { shouldFetchMetrics } from "$lib/streamMetricsLoader";
  import { groupStreamsByForwarder } from "$lib/groupStreams";
  import { StatusBadge, Card } from "@rusty-timer/shared-ui";

  // Per-stream rename state (keyed by stream_id)
  let renameValues: Record<string, string> = {};
  let renameBusy: Record<string, boolean> = {};
  let renameError: Record<string, string | null> = {};

  // Metrics fetching state
  let requestedMetricStreamIds = new Set<string>();
  let inFlightMetricStreamIds = new Set<string>();
  const METRICS_RETRY_DELAY_MS = 1000;
  let metricsRetryTimers: Record<string, ReturnType<typeof setTimeout>> = {};

  // Keep rename inputs in sync as streams arrive via SSE
  $: for (const s of $streamsStore) {
    if (!(s.stream_id in renameValues)) {
      renameValues[s.stream_id] = s.display_alias ?? "";
    }
  }

  // Fetch initial metrics for all streams
  $: for (const s of $streamsStore) {
    void maybeFetchMetrics(s.stream_id);
  }

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
  $: groupedStreams = groupStreamsByForwarder($streamsStore);

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
  let tick = 0;
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

  async function handleRename(streamId: string) {
    renameBusy[streamId] = true;
    renameError[streamId] = null;
    try {
      await api.renameStream(streamId, renameValues[streamId]);
      // SSE stream_updated event will update the store
    } catch (e) {
      renameError[streamId] = String(e);
    } finally {
      renameBusy[streamId] = false;
    }
  }

  function groupBorderStatus(
    stats: ReturnType<typeof groupStats>,
  ): "ok" | "warn" | "err" | undefined {
    if (stats.totalStreams === 0) return undefined;
    if (stats.onlineCount === 0) return "err";
    if (stats.onlineCount < stats.totalStreams) return "warn";
    return undefined;
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
  </div>

  {#each groupedStreams as group, groupIdx (group.forwarderId)}
    {@const stats = groupStats(group.streams, $metricsStore)}
    {@const border = groupBorderStatus(stats)}
    <div class="mb-6">
      <Card borderStatus={border} headerBg>
        <svelte:fragment slot="header">
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
            <a
              href="/forwarders/{group.forwarderId}/config"
              class="text-xs font-medium px-2.5 py-1 rounded-md text-accent no-underline bg-accent-bg hover:underline"
            >
              Configure
            </a>
          </div>
        </svelte:fragment>

        <div
          data-testid="stream-list"
          class="grid gap-3"
          style="grid-template-columns: repeat(auto-fill, minmax(420px, 1fr));"
        >
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
              </div>

              <div class="grid grid-cols-3 gap-3 mb-3">
                {#if $metricsStore[stream.stream_id]}
                  <div>
                    <p class="text-xs text-text-muted m-0">Reads</p>
                    <p
                      class="text-lg font-bold font-mono text-text-primary m-0"
                    >
                      {$metricsStore[
                        stream.stream_id
                      ].epoch_raw_count.toLocaleString()}
                    </p>
                  </div>
                  <div>
                    <p class="text-xs text-text-muted m-0">Chips</p>
                    <p
                      class="text-lg font-bold font-mono text-text-primary m-0"
                    >
                      {$metricsStore[
                        stream.stream_id
                      ].unique_chips.toLocaleString()}
                    </p>
                  </div>
                  <div>
                    <p class="text-xs text-text-muted m-0">Last read</p>
                    <p
                      class="text-lg font-bold font-mono text-text-primary m-0"
                    >
                      {tick !== undefined
                        ? timeSinceLastRead(stream.stream_id)
                        : ""}
                    </p>
                  </div>
                {:else}
                  <div class="col-span-3">
                    <p class="text-sm text-text-muted italic m-0">
                      Loading metrics…
                    </p>
                  </div>
                {/if}
              </div>

              <div class="flex items-center gap-3 text-xs text-text-muted mb-3">
                <span class="font-mono">{stream.reader_ip}</span>
                <span>&middot;</span>
                <span>epoch {stream.stream_epoch}</span>
              </div>

              <!-- Rename form -->
              <div class="flex gap-2 items-center">
                <input
                  data-testid="rename-input"
                  type="text"
                  bind:value={renameValues[stream.stream_id]}
                  placeholder="Display alias"
                  aria-label="Rename stream {stream.stream_id}"
                  class="flex-1 px-2 py-1 text-sm rounded-md border border-border bg-surface-0 text-text-primary placeholder:text-text-muted focus:outline-none focus:border-accent"
                />
                <button
                  data-testid="rename-btn"
                  on:click={() => handleRename(stream.stream_id)}
                  disabled={renameBusy[stream.stream_id]}
                  class="px-3 py-1 text-xs font-medium rounded-md bg-surface-2 border border-border text-text-secondary cursor-pointer hover:bg-surface-3 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {renameBusy[stream.stream_id] ? "Saving…" : "Rename"}
                </button>
              </div>

              {#if renameError[stream.stream_id]}
                <p class="text-xs text-status-err mt-1 m-0">
                  {renameError[stream.stream_id]}
                </p>
              {/if}
            </div>
          {/each}
        </div>
      </Card>
    </div>
  {/each}

  {#if $streamsStore.length === 0}
    <p class="text-sm text-text-muted">No streams found.</p>
  {/if}
</main>
