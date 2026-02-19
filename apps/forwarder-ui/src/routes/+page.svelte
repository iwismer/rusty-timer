<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import * as api from "$lib/api";
  import { initSSE, destroySSE } from "$lib/sse";
  import { waitForApplyResult } from "@rusty-timer/shared-ui/lib/update-flow";
  import { UpdateBanner, StatusBadge } from "@rusty-timer/shared-ui";
  import type { ForwarderStatus, ReaderStatus } from "$lib/api";

  let status: ForwarderStatus | null = null;
  let error: string | null = null;
  let updateVersion: string | null = null;
  let updateBusy = false;
  let sseConnected = false;

  async function loadAll() {
    try {
      status = await api.getStatus();
      const updateStatus = await api.getUpdateStatus().catch(() => null);
      if (updateStatus?.status === "downloaded" && updateStatus.version) {
        updateVersion = updateStatus.version;
      } else if (updateStatus?.status === "up_to_date") {
        updateVersion = null;
      }
    } catch (e) {
      error = String(e);
    }
  }

  async function handleApplyUpdate() {
    updateBusy = true;
    error = null;
    try {
      await api.applyUpdate();
      const result = await waitForApplyResult(() => api.getUpdateStatus());
      if (result.outcome === "applied") {
        updateVersion = null;
      } else if (result.outcome === "failed") {
        error = `Update failed: ${result.error}`;
      } else {
        error = "Update apply still in progress. Check status again shortly.";
      }
    } catch (e) {
      error = String(e);
    } finally {
      updateBusy = false;
    }
  }

  async function handleRestart() {
    try {
      await api.restart();
    } catch (e) {
      error = String(e);
    }
  }

  async function handleResetEpoch(readerIp: string) {
    try {
      await api.resetEpoch(readerIp);
    } catch (e) {
      error = String(e);
    }
  }

  function formatLastSeen(secs: number | null): string {
    if (secs === null) return "never";
    if (secs < 60) return `${secs}s ago`;
    if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
    return `${Math.floor(secs / 3600)}h ago`;
  }

  function readerBadgeState(state: string): "ok" | "warn" | "err" {
    if (state === "connected") return "ok";
    if (state === "connecting") return "warn";
    return "err";
  }

  onMount(() => {
    loadAll();
    initSSE({
      onStatusChanged: (data) => {
        if (status) {
          status = {
            ...status,
            ready: data.ready,
            uplink_connected: data.uplink_connected,
            restart_needed: data.restart_needed,
          };
        }
      },
      onReaderUpdated: (reader) => {
        if (status) {
          const readers = status.readers.map((r) =>
            r.ip === reader.ip ? reader : r,
          );
          status = { ...status, readers };
        }
      },
      onLogEntry: () => {},
      onResync: () => loadAll(),
      onConnectionChange: (connected) => {
        sseConnected = connected;
      },
      onUpdateAvailable: (version) => {
        updateVersion = version;
      },
    });
  });

  onDestroy(() => destroySSE());
</script>

<main>
  <h1>Forwarder Status</h1>

  {#if updateVersion}
    <UpdateBanner
      version={updateVersion}
      busy={updateBusy}
      onApply={handleApplyUpdate}
    />
  {/if}

  {#if status?.restart_needed}
    <div class="restart-banner">
      Configuration changed. Restart to apply.
      <button on:click={handleRestart}>Restart Now</button>
    </div>
  {/if}

  {#if error}
    <p class="error">{error}</p>
  {/if}

  {#if status}
    <section>
      <p>Version: {status.version}</p>
      <p>Forwarder ID: <code>{status.forwarder_id}</code></p>
      <p>
        Readiness:
        <StatusBadge
          label={status.ready ? "ready" : "not ready"}
          state={status.ready ? "ok" : "err"}
        />
        {#if status.ready_reason}
          <span class="reason">({status.ready_reason})</span>
        {/if}
      </p>
      <p>
        Uplink:
        <StatusBadge
          label={status.uplink_connected ? "connected" : "disconnected"}
          state={status.uplink_connected ? "ok" : "err"}
        />
      </p>
    </section>

    <section>
      <h2>Readers</h2>
      {#if status.readers.length === 0}
        <p>No readers configured.</p>
      {:else}
        <table>
          <thead>
            <tr>
              <th>Reader IP</th>
              <th>Status</th>
              <th>Reads (session)</th>
              <th>Reads (total)</th>
              <th>Last seen</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {#each status.readers as reader}
              <tr>
                <td>{reader.ip}</td>
                <td>
                  <StatusBadge
                    label={reader.state}
                    state={readerBadgeState(reader.state)}
                  />
                </td>
                <td>{reader.reads_session}</td>
                <td>{reader.reads_total}</td>
                <td>{formatLastSeen(reader.last_seen_secs)}</td>
                <td>
                  <button
                    class="small"
                    on:click={() => handleResetEpoch(reader.ip)}
                  >
                    Reset Epoch
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </section>
  {:else if !error}
    <p>Loading...</p>
  {/if}
</main>

<style>
  table {
    border-collapse: collapse;
    width: 100%;
  }
  th,
  td {
    text-align: left;
    padding: 0.4rem 0.6rem;
    border-bottom: 1px solid #ddd;
  }
  th {
    font-weight: 600;
  }
  code {
    background: #f5f5f5;
    padding: 0.15rem 0.3rem;
    border-radius: 3px;
    font-size: 0.9em;
  }
  .error {
    color: var(--color-err);
  }
  .reason {
    color: #666;
    font-size: 0.9em;
  }
  .restart-banner {
    background: var(--color-warn-bg);
    color: var(--color-warn);
    border: 1px solid #ffc107;
    padding: 0.75rem 1rem;
    border-radius: 4px;
    margin-bottom: 1rem;
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .small {
    padding: 0.2rem 0.5rem;
    font-size: 0.85em;
  }
</style>
