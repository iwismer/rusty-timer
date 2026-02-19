<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import * as api from "$lib/api";
  import { initSSE, destroySSE } from "$lib/sse";
  import { waitForApplyResult } from "@rusty-timer/shared-ui/lib/update-flow";
  import {
    UpdateBanner,
    StatusBadge,
    Card,
    AlertBanner,
  } from "@rusty-timer/shared-ui";
  import type { ForwarderStatus, ReaderStatus } from "$lib/api";

  let status: ForwarderStatus | null = null;
  let error: string | null = null;
  let updateVersion: string | null = null;
  let updateBusy = false;
  let sseConnected = false;

  async function loadAll() {
    error = null;
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
        if (!connected) {
          status = null;
        }
      },
      onUpdateAvailable: (version) => {
        updateVersion = version;
      },
    });
  });

  onDestroy(() => destroySSE());
</script>

<main class="max-w-[900px] mx-auto px-6 py-6">
  {#if updateVersion}
    <div class="mb-4">
      <UpdateBanner
        version={updateVersion}
        busy={updateBusy}
        onApply={handleApplyUpdate}
      />
    </div>
  {/if}

  {#if status?.restart_needed}
    <div class="mb-4">
      <AlertBanner
        variant="warn"
        message="Configuration changed. Restart to apply."
        actionLabel="Restart Now"
        onAction={handleRestart}
      />
    </div>
  {/if}

  {#if error}
    <div class="mb-4">
      <AlertBanner variant="err" message={error} />
    </div>
  {/if}

  <h1 class="text-xl font-bold text-text-primary mb-6">Forwarder Status</h1>

  {#if status}
    <div class="grid grid-cols-3 gap-4 mb-6">
      <Card>
        <p class="text-xs text-text-muted m-0">Forwarder ID</p>
        <p class="text-sm font-mono font-medium text-text-primary m-0 mt-1">
          {status.forwarder_id}
        </p>
        <p class="text-xs text-text-muted mt-2 m-0">v{status.version}</p>
      </Card>
      <Card>
        <p class="text-xs text-text-muted m-0">Readiness</p>
        <div class="mt-1 flex items-center gap-2">
          <StatusBadge
            label={status.ready ? "ready" : "not ready"}
            state={status.ready ? "ok" : "err"}
          />
          {#if status.ready_reason}
            <span class="text-xs text-text-muted">
              ({status.ready_reason})
            </span>
          {/if}
        </div>
      </Card>
      <Card>
        <p class="text-xs text-text-muted m-0">Uplink</p>
        <div class="mt-1">
          <StatusBadge
            label={status.uplink_connected ? "connected" : "disconnected"}
            state={status.uplink_connected ? "ok" : "err"}
          />
        </div>
      </Card>
    </div>

    <Card headerBg>
      <svelte:fragment slot="header">
        <h2 class="text-sm font-semibold text-text-primary m-0">Readers</h2>
        <span class="ml-auto text-xs text-text-muted">
          {status.readers.length} configured
        </span>
      </svelte:fragment>

      {#if status.readers.length === 0}
        <p class="text-sm text-text-muted m-0">No readers configured.</p>
      {:else}
        <div class="overflow-x-auto -mx-4 -mb-4">
          <table class="w-full text-sm border-collapse">
            <thead>
              <tr class="border-b border-border">
                <th
                  class="text-left px-4 py-2.5 text-xs font-medium text-text-secondary"
                >
                  Reader IP
                </th>
                <th
                  class="text-left px-4 py-2.5 text-xs font-medium text-text-secondary"
                >
                  Status
                </th>
                <th
                  class="text-right px-4 py-2.5 text-xs font-medium text-text-secondary"
                >
                  Reads (session)
                </th>
                <th
                  class="text-right px-4 py-2.5 text-xs font-medium text-text-secondary"
                >
                  Reads (total)
                </th>
                <th
                  class="text-left px-4 py-2.5 text-xs font-medium text-text-secondary"
                >
                  Last seen
                </th>
                <th class="px-4 py-2.5"></th>
              </tr>
            </thead>
            <tbody>
              {#each status.readers as reader}
                <tr class="border-b border-border last:border-b-0">
                  <td class="px-4 py-2.5 font-mono text-text-primary">
                    {reader.ip}
                  </td>
                  <td class="px-4 py-2.5">
                    <StatusBadge
                      label={reader.state}
                      state={readerBadgeState(reader.state)}
                    />
                  </td>
                  <td
                    class="px-4 py-2.5 text-right font-mono text-text-primary"
                  >
                    {reader.reads_session.toLocaleString()}
                  </td>
                  <td
                    class="px-4 py-2.5 text-right font-mono text-text-primary"
                  >
                    {reader.reads_total.toLocaleString()}
                  </td>
                  <td class="px-4 py-2.5 text-xs text-text-secondary">
                    {formatLastSeen(reader.last_seen_secs)}
                  </td>
                  <td class="px-4 py-2.5 text-right">
                    <button
                      on:click={() => handleResetEpoch(reader.ip)}
                      class="px-2 py-1 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2"
                    >
                      Reset Epoch
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </Card>
  {:else if !sseConnected}
    <AlertBanner variant="err" message="Disconnected from forwarder." />
  {:else if !error}
    <p class="text-sm text-text-muted">Loading...</p>
  {/if}
</main>
