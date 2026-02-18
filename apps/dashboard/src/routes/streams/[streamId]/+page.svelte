<script lang="ts">
  import { onDestroy } from "svelte";
  import { page } from "$app/stores";
  import * as api from "$lib/api";
  import { streamsStore, metricsStore, setMetrics } from "$lib/stores";
  import { shouldFetchMetrics } from "$lib/streamMetricsLoader";

  let resetResult: string | null = null;
  let resetBusy = false;
  let requestedMetricStreamIds = new Set<string>();
  let inFlightMetricStreamIds = new Set<string>();

  $: streamId = $page.params.streamId;
  $: stream = $streamsStore.find((s) => s.stream_id === streamId) ?? null;
  $: metrics = $metricsStore[streamId] ?? null;
  $: void maybeFetchMetrics(streamId);

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

  let timeSinceLastRead: string = "N/A";
  let timerHandle: ReturnType<typeof setInterval> | null = null;

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

  $: {
    if (timerHandle) clearInterval(timerHandle);
    updateTimeSinceLastRead();
    timerHandle = setInterval(updateTimeSinceLastRead, 1000);
  }

  onDestroy(() => {
    if (timerHandle) clearInterval(timerHandle);
  });
</script>

<main>
  <a data-testid="back-link" href="/">← Back to stream list</a>

  <h1 data-testid="stream-detail-heading">
    Stream Detail
    {#if stream}
      — {stream.display_alias ?? `${stream.forwarder_id} / ${stream.reader_ip}`}
    {:else}
      — {streamId}
    {/if}
  </h1>

  {#if stream}
    <section class="meta-section">
      <p><strong>Stream ID:</strong> {stream.stream_id}</p>
      <p><strong>Forwarder:</strong> {stream.forwarder_id}</p>
      <p><strong>Reader IP:</strong> {stream.reader_ip}</p>
      <p>
        <strong>Status:</strong>
        {#if stream.online}
          <span class="badge online">online</span>
        {:else}
          <span class="badge offline">offline</span>
        {/if}
      </p>
      <p><strong>Epoch:</strong> {stream.stream_epoch}</p>
      <p>
        <strong>Created:</strong>
        {new Date(stream.created_at).toLocaleString()}
      </p>
    </section>
  {/if}

  <!-- Metrics -->
  <section data-testid="metrics-section">
    <h2>Metrics</h2>
    {#if !metrics}
      <p>Loading metrics…</p>
    {:else}
      <table>
        <tbody>
          <tr>
            <td>Raw count</td>
            <td data-testid="metric-raw-count">{metrics.raw_count}</td>
          </tr>
          <tr>
            <td>Dedup count</td>
            <td data-testid="metric-dedup-count">{metrics.dedup_count}</td>
          </tr>
          <tr>
            <td>Retransmit count</td>
            <td data-testid="metric-retransmit-count"
              >{metrics.retransmit_count}</td
            >
          </tr>
          <tr>
            <td>Lag</td>
            <td data-testid="metric-lag">{formatLag(metrics.lag)}</td>
          </tr>
          <tr>
            <td>Backlog</td>
            <td data-testid="metric-backlog">{metrics.backlog}</td>
          </tr>
          <tr>
            <td
              colspan="2"
              style="border-bottom: 2px solid #ccc; padding-top: 0.75rem;"
            >
              <strong>Current Epoch</strong>
            </td>
          </tr>
          <tr>
            <td>Raw count (epoch)</td>
            <td data-testid="metric-epoch-raw-count"
              >{metrics.epoch_raw_count}</td
            >
          </tr>
          <tr>
            <td>Dedup count (epoch)</td>
            <td data-testid="metric-epoch-dedup-count"
              >{metrics.epoch_dedup_count}</td
            >
          </tr>
          <tr>
            <td>Retransmit count (epoch)</td>
            <td data-testid="metric-epoch-retransmit-count"
              >{metrics.epoch_retransmit_count}</td
            >
          </tr>
          <tr>
            <td>Unique chips</td>
            <td data-testid="metric-unique-chips">{metrics.unique_chips}</td>
          </tr>
          <tr>
            <td>Last read</td>
            <td data-testid="metric-last-read">
              {metrics.epoch_last_received_at
                ? new Date(metrics.epoch_last_received_at).toLocaleString()
                : "N/A (no events in epoch)"}
            </td>
          </tr>
          <tr>
            <td>Time since last read</td>
            <td data-testid="metric-time-since-last-read"
              >{timeSinceLastRead}</td
            >
          </tr>
        </tbody>
      </table>
    {/if}
  </section>

  <!-- Export links -->
  <section data-testid="export-section">
    <h2>Export</h2>
    <p>
      Downloads contain all canonical (deduplicated) events, ordered by epoch
      and sequence.
    </p>
    <ul>
      <li>
        <a
          data-testid="export-raw-link"
          href={api.exportRawUrl(streamId)}
          download
        >
          Download export.txt (one raw_read_line per row)
        </a>
      </li>
      <li>
        <a
          data-testid="export-csv-link"
          href={api.exportCsvUrl(streamId)}
          download
        >
          Download export.csv (stream_epoch, seq, reader_timestamp,
          raw_read_line, read_type)
        </a>
      </li>
    </ul>
  </section>

  <!-- Epoch reset -->
  <section class="actions-section">
    <h2>Actions</h2>
    <button
      data-testid="reset-epoch-btn"
      on:click={handleResetEpoch}
      disabled={resetBusy}
      class="danger"
    >
      {resetBusy ? "Sending…" : "Reset Epoch"}
    </button>
    <p class="hint">
      Sends an epoch-reset command to the connected forwarder. Only works while
      the forwarder is connected; returns 409 otherwise.
    </p>
    {#if resetResult}
      <p
        data-testid="reset-epoch-result"
        class:success={!resetResult.startsWith("Error")}
        class:error={resetResult.startsWith("Error")}
      >
        {resetResult}
      </p>
    {/if}
  </section>
</main>

<style>
  main {
    max-width: 900px;
    margin: 0 auto;
    padding: 1rem;
    font-family: sans-serif;
  }
  a {
    color: #0070f3;
  }
  a:hover {
    text-decoration: underline;
  }
  section {
    margin-bottom: 2rem;
    border: 1px solid #ccc;
    padding: 1rem;
    border-radius: 4px;
  }
  h2 {
    margin-top: 0;
  }
  table {
    border-collapse: collapse;
    width: 100%;
  }
  td {
    padding: 0.35rem 0.75rem;
    border-bottom: 1px solid #eee;
  }
  td:first-child {
    font-weight: bold;
    width: 40%;
  }
  ul {
    padding-left: 1.25rem;
  }
  li {
    margin-bottom: 0.5rem;
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
  button {
    padding: 0.4rem 1rem;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  button.danger {
    background: #dc3545;
    color: white;
    border: none;
    border-radius: 4px;
  }
  button.danger:hover:not(:disabled) {
    background: #c82333;
  }
  .hint {
    font-size: 0.8em;
    color: #666;
    margin-top: 0.35rem;
  }
  .error {
    color: red;
  }
  .success {
    color: green;
  }
  .meta-section {
    border: 1px solid #ccc;
    padding: 1rem;
    border-radius: 4px;
    margin-bottom: 1.5rem;
  }
  .actions-section {
    border: 1px solid #ccc;
    padding: 1rem;
    border-radius: 4px;
    margin-bottom: 1.5rem;
  }
</style>
