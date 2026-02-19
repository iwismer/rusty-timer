<script lang="ts">
  import { onDestroy } from "svelte";
  import * as api from "$lib/api";
  import { streamsStore, metricsStore, setMetrics } from "$lib/stores";
  import { shouldFetchMetrics } from "$lib/streamMetricsLoader";
  import { groupStreamsByForwarder } from "$lib/groupStreams";

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
</script>

<main>
  <h1 data-testid="streams-heading">Dashboard – Streams</h1>

  {#each groupedStreams as group (group.forwarderId)}
    {@const stats = groupStats(group.streams, $metricsStore)}
    <section class="forwarder-group">
      <div class="forwarder-header">
        <h2>{group.displayName}</h2>
        <span class="group-stats">
          {stats.totalRaw} reads · {stats.totalChips} chips · {stats.onlineCount}/{stats.totalStreams}
          online
        </span>
        <a href="/forwarders/{group.forwarderId}/config" class="configure-link"
          >Configure</a
        >
      </div>

      <ul data-testid="stream-list">
        {#each group.streams as stream (stream.stream_id)}
          <li data-testid="stream-item">
            <div class="stream-header">
              <a
                data-testid="stream-detail-link"
                href="/streams/{stream.stream_id}"
              >
                {#if stream.display_alias}
                  <strong>{stream.display_alias}</strong>
                {:else}
                  <strong>{stream.reader_ip}</strong>
                {/if}
              </a>
              {#if stream.online}
                <span data-testid="stream-online-badge" class="badge online"
                  >online</span
                >
              {:else}
                <span data-testid="stream-offline-badge" class="badge offline"
                  >offline</span
                >
              {/if}
            </div>

            <div class="stream-stats">
              {#if $metricsStore[stream.stream_id]}
                <span
                  >Reads: {$metricsStore[stream.stream_id]
                    .epoch_raw_count}</span
                >
                <span
                  >Chips: {$metricsStore[stream.stream_id].unique_chips}</span
                >
                <span
                  >Last read: {tick !== undefined
                    ? timeSinceLastRead(stream.stream_id)
                    : ""}</span
                >
              {:else}
                <span class="loading">Loading metrics…</span>
              {/if}
            </div>

            <div class="stream-meta">
              <span>reader: {stream.reader_ip}</span>
              <span>epoch: {stream.stream_epoch}</span>
            </div>

            <!-- Rename form -->
            <div class="rename-row">
              <input
                data-testid="rename-input"
                type="text"
                bind:value={renameValues[stream.stream_id]}
                placeholder="Display alias"
                aria-label="Rename stream {stream.stream_id}"
              />
              <button
                data-testid="rename-btn"
                on:click={() => handleRename(stream.stream_id)}
                disabled={renameBusy[stream.stream_id]}
              >
                {renameBusy[stream.stream_id] ? "Saving…" : "Rename"}
              </button>
            </div>

            {#if renameError[stream.stream_id]}
              <p class="error">{renameError[stream.stream_id]}</p>
            {/if}
          </li>
        {/each}
      </ul>
    </section>
  {/each}

  {#if $streamsStore.length === 0}
    <p>No streams found.</p>
  {/if}
</main>

<style>
  main {
    max-width: 900px;
    margin: 0 auto;
    padding: 1rem;
    font-family: sans-serif;
  }
  .forwarder-group {
    border: 1px solid #ddd;
    border-radius: 6px;
    padding: 1rem;
    margin-bottom: 1.5rem;
  }
  .forwarder-header {
    display: flex;
    align-items: baseline;
    gap: 1rem;
    margin-bottom: 0.75rem;
  }
  .forwarder-header h2 {
    margin: 0;
    font-size: 1.2rem;
  }
  .group-stats {
    font-size: 0.85em;
    color: #666;
  }
  .configure-link {
    font-size: 0.85em;
    margin-left: auto;
  }
  ul {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  li {
    border: 1px solid #ccc;
    padding: 0.75rem 1rem;
    margin-bottom: 0.75rem;
    border-radius: 4px;
  }
  .stream-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.4rem;
  }
  .stream-stats {
    display: flex;
    gap: 1rem;
    font-size: 0.85em;
    margin-bottom: 0.4rem;
  }
  .stream-stats .loading {
    color: #999;
    font-style: italic;
  }
  .stream-meta {
    font-size: 0.8em;
    color: #666;
    display: flex;
    gap: 1rem;
    margin-bottom: 0.4rem;
  }
  .rename-row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin-top: 0.4rem;
  }
  .rename-row input {
    flex: 1;
    padding: 0.25rem 0.5rem;
  }
  .badge {
    font-size: 0.75em;
    padding: 0.15rem 0.5rem;
    border-radius: 3px;
    font-weight: bold;
  }
  .online {
    background: #d4edda;
    color: #155724;
  }
  .offline {
    background: #f8d7da;
    color: #721c24;
  }
  a {
    text-decoration: none;
    color: #0070f3;
  }
  a:hover {
    text-decoration: underline;
  }
  button {
    padding: 0.25rem 0.75rem;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .error {
    color: red;
    margin: 0.25rem 0;
    font-size: 0.85em;
  }
</style>
