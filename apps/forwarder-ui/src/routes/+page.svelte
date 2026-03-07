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
    LogViewer,
  } from "@rusty-timer/shared-ui";
  import type { ForwarderStatus } from "$lib/api";
  import {
    formatLastSeen,
    readerBadgeState,
    readerConnectionSummary,
    formatClockDrift,
  } from "$lib/status-view-model";
  import { pushLogEntry } from "$lib/log-buffer";

  let status = $state<ForwarderStatus | null>(null);
  let error = $state<string | null>(null);
  let updateVersion = $state<string | null>(null);
  let updateStatus = $state<"available" | "downloaded" | null>(null);
  let updateBusy = $state(false);
  let sseConnected = $state(false);
  let logs = $state<string[]>([]);
  let epochNameDrafts = $state<Record<string, string>>({});
  let epochNameBusy = $state<Record<string, boolean>>({});
  let epochNameFeedback = $state<
    Record<string, { kind: "ok" | "err"; message: string } | undefined>
  >({});
  let resetEpochFeedback = $state<
    Record<string, { kind: "ok" | "err"; message: string } | undefined>
  >({});
  let expandedReader = $state<string | null>(null);
  let readerInfoMap = $state<Record<string, api.ReaderInfo>>({});
  let controlBusy = $state<Record<string, boolean>>({});
  let controlFeedback = $state<
    Record<string, { kind: "ok" | "err"; message: string } | undefined>
  >({});

  const btnPrimary =
    "px-3 py-1.5 text-sm font-medium rounded-md text-white bg-accent border-none cursor-pointer hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed";

  let readersSummary = $derived(
    status
      ? readerConnectionSummary(status.readers)
      : { connected: 0, configured: 0, label: "0 connected / 0 configured" },
  );

  async function loadAll() {
    error = null;
    try {
      status = await api.getStatus();
      if (status) {
        for (const r of status.readers) {
          if (r.reader_info) {
            readerInfoMap = { ...readerInfoMap, [r.ip]: r.reader_info };
          }
        }
      }
      const [usResult, logsResp] = await Promise.allSettled([
        api.getUpdateStatus(),
        api.getLogs(),
      ]);
      if (usResult.status === "fulfilled") {
        const us = usResult.value;
        if (
          (us.status === "downloaded" || us.status === "available") &&
          us.version
        ) {
          updateVersion = us.version;
          updateStatus = us.status;
        } else {
          updateVersion = null;
          updateStatus = null;
        }
      }
      if (logsResp.status === "fulfilled") {
        logs = logsResp.value.entries;
      }
    } catch (e) {
      error = String(e);
    }
  }

  async function handleDownloadUpdate() {
    updateBusy = true;
    error = null;
    try {
      const result = await api.downloadUpdate();
      if (result.status === "downloaded") {
        updateVersion = result.version ?? null;
        updateStatus = "downloaded";
      } else if (result.status === "failed") {
        error = result.error ?? "Download failed.";
      } else {
        error = "No downloadable update available.";
      }
    } catch (e) {
      error = String(e);
    } finally {
      updateBusy = false;
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
        updateStatus = null;
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
    resetEpochFeedback = { ...resetEpochFeedback, [readerIp]: undefined };
    try {
      const result = await api.resetEpoch(readerIp);
      resetEpochFeedback = {
        ...resetEpochFeedback,
        [readerIp]: {
          kind: "ok",
          message: `Advanced to epoch ${result.new_epoch}.`,
        },
      };
    } catch (e) {
      const msg = String(e);
      error = msg;
      resetEpochFeedback = {
        ...resetEpochFeedback,
        [readerIp]: { kind: "err", message: `Failed to advance epoch: ${msg}` },
      };
    }
  }

  function updateEpochNameDraft(readerIp: string, value: string) {
    epochNameDrafts = { ...epochNameDrafts, [readerIp]: value };
  }

  function setEpochNameBusy(readerIp: string, busy: boolean) {
    epochNameBusy = { ...epochNameBusy, [readerIp]: busy };
  }

  async function handleSetCurrentEpochName(
    readerIp: string,
    name: string | null,
  ) {
    setEpochNameBusy(readerIp, true);
    error = null;
    epochNameFeedback = { ...epochNameFeedback, [readerIp]: undefined };
    try {
      await api.setCurrentEpochName(readerIp, name);
      if (name === null) {
        epochNameDrafts = { ...epochNameDrafts, [readerIp]: "" };
      }
      epochNameFeedback = {
        ...epochNameFeedback,
        [readerIp]: {
          kind: "ok",
          message: name === null ? "Epoch name cleared." : "Epoch name saved.",
        },
      };
    } catch (e) {
      const msg = String(e);
      error = msg;
      epochNameFeedback = {
        ...epochNameFeedback,
        [readerIp]: {
          kind: "err",
          message: `Failed to update epoch name: ${msg}`,
        },
      };
    } finally {
      setEpochNameBusy(readerIp, false);
    }
  }

  function toggleReaderExpand(ip: string) {
    expandedReader = expandedReader === ip ? null : ip;
  }

  async function handleSyncClock(ip: string) {
    controlBusy = { ...controlBusy, [ip]: true };
    controlFeedback = { ...controlFeedback, [ip]: undefined };
    try {
      const result = await api.syncReaderClock(ip);
      controlFeedback = {
        ...controlFeedback,
        [ip]: { kind: "ok", message: `Clock synced: ${result.reader_clock}` },
      };
    } catch (e) {
      controlFeedback = {
        ...controlFeedback,
        [ip]: { kind: "err", message: `Sync failed: ${e}` },
      };
    } finally {
      controlBusy = { ...controlBusy, [ip]: false };
    }
  }

  async function handleSetReadMode(ip: string, mode: string) {
    controlBusy = { ...controlBusy, [ip]: true };
    controlFeedback = { ...controlFeedback, [ip]: undefined };
    try {
      await api.setReadMode(ip, mode);
      controlFeedback = {
        ...controlFeedback,
        [ip]: { kind: "ok", message: `Mode set to ${mode}` },
      };
    } catch (e) {
      controlFeedback = {
        ...controlFeedback,
        [ip]: { kind: "err", message: `Set mode failed: ${e}` },
      };
    } finally {
      controlBusy = { ...controlBusy, [ip]: false };
    }
  }

  async function handleRefreshReader(ip: string) {
    controlBusy = { ...controlBusy, [ip]: true };
    try {
      await api.refreshReader(ip);
    } catch (e) {
      controlFeedback = {
        ...controlFeedback,
        [ip]: { kind: "err", message: `Refresh failed: ${e}` },
      };
    } finally {
      controlBusy = { ...controlBusy, [ip]: false };
    }
  }

  async function handleClearRecords(ip: string) {
    controlBusy = { ...controlBusy, [ip]: true };
    controlFeedback = { ...controlFeedback, [ip]: undefined };
    try {
      await api.clearReaderRecords(ip);
      controlFeedback = {
        ...controlFeedback,
        [ip]: { kind: "ok", message: "Records cleared" },
      };
    } catch (e) {
      controlFeedback = {
        ...controlFeedback,
        [ip]: { kind: "err", message: `Clear failed: ${e}` },
      };
    } finally {
      controlBusy = { ...controlBusy, [ip]: false };
    }
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
      onLogEntry: (entry) => {
        logs = pushLogEntry(logs, entry);
      },
      onReaderInfoUpdated: (data) => {
        const { ip, ...info } = data;
        readerInfoMap = {
          ...readerInfoMap,
          [ip]: { ...readerInfoMap[ip], ...info },
        };
      },
      onResync: () => loadAll(),
      onConnectionChange: (connected) => {
        sseConnected = connected;
        if (!connected) {
          status = null;
        }
      },
      onUpdateStatusChanged: (us) => {
        if (
          (us.status === "available" || us.status === "downloaded") &&
          us.version
        ) {
          updateVersion = us.version;
          updateStatus = us.status;
        } else {
          updateVersion = null;
          updateStatus = null;
        }
      },
    });
  });

  onDestroy(() => destroySSE());
</script>

<main class="max-w-[900px] mx-auto px-6 py-6">
  {#if updateVersion && updateStatus}
    <div class="mb-4">
      <UpdateBanner
        version={updateVersion}
        status={updateStatus}
        busy={updateBusy}
        onDownload={handleDownloadUpdate}
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

  <h1 class="text-xl font-bold text-text-primary mb-6">Forwarder</h1>

  {#if status}
    <div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-6">
      <Card title="Status">
        <dl class="grid gap-2 text-sm" style="grid-template-columns: auto 1fr;">
          <dt class="text-text-muted">Forwarder ID</dt>
          <dd class="font-mono text-text-primary">{status.forwarder_id}</dd>
          <dt class="text-text-muted">Version</dt>
          <dd class="font-mono text-text-primary">{status.version}</dd>
          <dt class="text-text-muted">Readiness</dt>
          <dd class="flex items-center gap-2">
            <StatusBadge
              label={status.ready ? "ready" : "not ready"}
              state={status.ready ? "ok" : "err"}
            />
            {#if status.ready_reason}
              <span class="text-xs text-text-muted">
                ({status.ready_reason})
              </span>
            {/if}
          </dd>
        </dl>
      </Card>
      <Card title="Service">
        <dl class="grid gap-2 text-sm" style="grid-template-columns: auto 1fr;">
          <dt class="text-text-muted">Uplink</dt>
          <dd>
            <StatusBadge
              label={status.uplink_connected ? "connected" : "disconnected"}
              state={status.uplink_connected ? "ok" : "err"}
            />
          </dd>
          <dt class="text-text-muted">Restart Needed</dt>
          <dd>
            <StatusBadge
              label={status.restart_needed ? "pending" : "none"}
              state={status.restart_needed ? "warn" : "ok"}
            />
          </dd>
        </dl>
        <div class="flex gap-2 mt-3 pt-3 border-t border-border">
          <button
            class={btnPrimary}
            onclick={handleRestart}
            disabled={!status.restart_needed}
          >
            Restart Now
          </button>
          <span class="text-xs text-text-muted self-center">
            Applies recent configuration changes.
          </span>
        </div>
      </Card>
    </div>

    <Card headerBg>
      {#snippet header()}
        <h2 class="text-sm font-semibold text-text-primary m-0">Readers</h2>
        <span class="ml-auto text-xs text-text-muted">
          {readersSummary.label}
        </span>
      {/snippet}

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
                  class="text-right px-4 py-2.5 text-xs font-medium text-text-secondary"
                >
                  Local Port
                </th>
                <th
                  class="text-left px-4 py-2.5 text-xs font-medium text-text-secondary"
                >
                  Last seen
                </th>
                <th
                  class="text-left px-4 py-2.5 text-xs font-medium text-text-secondary"
                >
                  Current epoch name
                </th>
                <th class="px-4 py-2.5"></th>
              </tr>
            </thead>
            <tbody>
              {#each status.readers as reader}
                <tr
                  class="border-b border-border last:border-b-0 cursor-pointer hover:bg-surface-1"
                  onclick={() => toggleReaderExpand(reader.ip)}
                >
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
                  <td
                    class="px-4 py-2.5 text-right font-mono text-text-primary"
                  >
                    {reader.local_port}
                  </td>
                  <td class="px-4 py-2.5 text-xs text-text-secondary">
                    {formatLastSeen(reader.last_seen_secs)}
                  </td>
                  <td class="px-4 py-2.5">
                    <div class="flex flex-col gap-1">
                      {#if reader.current_epoch_name}
                        <span class="text-xs text-text-muted font-mono">
                          Active: {reader.current_epoch_name}
                        </span>
                      {/if}
                      <div class="flex items-center gap-2">
                        <input
                          type="text"
                          class="w-48 px-2 py-1 text-xs rounded-md bg-surface-0 text-text-primary border border-border"
                          placeholder="Set name"
                          value={epochNameDrafts[reader.ip] ?? ""}
                          oninput={(event) =>
                            updateEpochNameDraft(
                              reader.ip,
                              (event.currentTarget as HTMLInputElement).value,
                            )}
                          disabled={epochNameBusy[reader.ip] === true}
                        />
                        <button
                          onclick={() =>
                            handleSetCurrentEpochName(
                              reader.ip,
                              (epochNameDrafts[reader.ip] ?? "").trim() || null,
                            )}
                          class="px-2 py-1 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50 disabled:cursor-not-allowed"
                          disabled={epochNameBusy[reader.ip] === true}
                        >
                          Save
                        </button>
                      </div>
                      {#if epochNameFeedback[reader.ip]}
                        {@const feedback = epochNameFeedback[reader.ip]}
                        {#if feedback}
                          <span
                            class={`text-xs ${
                              feedback.kind === "ok"
                                ? "text-status-ok"
                                : "text-status-err"
                            }`}
                          >
                            {feedback.message}
                          </span>
                        {/if}
                      {/if}
                    </div>
                  </td>
                  <td class="px-4 py-2.5 text-right">
                    <div class="flex flex-col items-end gap-1">
                      <button
                        onclick={() => handleResetEpoch(reader.ip)}
                        class="px-2 py-1 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2"
                      >
                        Advance Epoch
                      </button>
                      {#if resetEpochFeedback[reader.ip]}
                        {@const rf = resetEpochFeedback[reader.ip]}
                        {#if rf}
                          <span
                            class={`text-xs ${rf.kind === "ok" ? "text-status-ok" : "text-status-err"}`}
                          >
                            {rf.message}
                          </span>
                        {/if}
                      {/if}
                    </div>
                  </td>
                </tr>
                {#if expandedReader === reader.ip}
                  {@const info = readerInfoMap[reader.ip]}
                  <tr class="border-b border-border bg-surface-0">
                    <td colspan="8" class="px-6 py-4">
                      <div
                        class="grid grid-cols-2 gap-x-8 gap-y-2 text-sm mb-4"
                      >
                        <div>
                          <span class="text-text-muted">Firmware:</span>
                          <span class="font-mono ml-2"
                            >{info?.fw_version ?? "\u2014"}</span
                          >
                        </div>
                        <div>
                          <span class="text-text-muted">Hardware:</span>
                          <span class="font-mono ml-2"
                            >{info?.hw_code != null
                              ? `0x${info.hw_code.toString(16)}`
                              : "\u2014"}</span
                          >
                        </div>
                        <div>
                          <span class="text-text-muted">Banner:</span>
                          <span class="font-mono ml-2 text-xs"
                            >{info?.banner ?? "\u2014"}</span
                          >
                        </div>
                        <div>
                          <span class="text-text-muted">Unique Tags:</span>
                          <span class="font-mono ml-2"
                            >{info?.unique_tag_count ?? "\u2014"}</span
                          >
                        </div>
                        <div>
                          <span class="text-text-muted">Reader Clock:</span>
                          <span class="font-mono ml-2"
                            >{info?.reader_clock ?? "\u2014"}</span
                          >
                        </div>
                        <div>
                          <span class="text-text-muted">Clock Drift:</span>
                          <span class="font-mono ml-2"
                            >{formatClockDrift(info?.clock_drift_ms)}</span
                          >
                        </div>
                        <div>
                          <span class="text-text-muted">Read Mode:</span>
                          <span class="font-mono ml-2"
                            >{info?.read_mode ?? "\u2014"}</span
                          >
                        </div>
                        <div>
                          <span class="text-text-muted">Stored Pages:</span>
                          <span class="font-mono ml-2"
                            >{info?.stored_record_pages ?? "\u2014"}</span
                          >
                        </div>
                      </div>
                      <div
                        class="flex items-center gap-3 pt-3 border-t border-border"
                      >
                        <button
                          class={btnPrimary}
                          onclick={(e) => {
                            e.stopPropagation();
                            handleSyncClock(reader.ip);
                          }}
                          disabled={controlBusy[reader.ip] ||
                            reader.state !== "connected"}>Sync Clock</button
                        >
                        <select
                          class="px-2 py-1.5 text-sm rounded-md bg-surface-0 text-text-primary border border-border"
                          onchange={(e) => {
                            e.stopPropagation();
                            handleSetReadMode(
                              reader.ip,
                              (e.currentTarget as HTMLSelectElement).value,
                            );
                          }}
                          disabled={controlBusy[reader.ip] ||
                            reader.state !== "connected"}
                        >
                          <option
                            value="raw"
                            selected={info?.read_mode === "raw"}>Raw</option
                          >
                          <option
                            value="fsls"
                            selected={info?.read_mode === "fsls"}
                            >First/Last Seen</option
                          >
                        </select>
                        <button
                          class="px-3 py-1.5 text-sm rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
                          onclick={(e) => {
                            e.stopPropagation();
                            handleRefreshReader(reader.ip);
                          }}
                          disabled={controlBusy[reader.ip] ||
                            reader.state !== "connected"}>Refresh</button
                        >
                        <button
                          class="px-3 py-1.5 text-sm rounded-md bg-red-600 text-white border-none cursor-pointer hover:bg-red-700 disabled:opacity-50"
                          onclick={(e) => {
                            e.stopPropagation();
                            handleClearRecords(reader.ip);
                          }}
                          disabled={controlBusy[reader.ip] ||
                            reader.state !== "connected"}>Clear Records</button
                        >
                        {#if controlFeedback[reader.ip]}
                          {@const fb = controlFeedback[reader.ip]}
                          {#if fb}
                            <span
                              class={`text-xs ${fb.kind === "ok" ? "text-status-ok" : "text-status-err"}`}
                              >{fb.message}</span
                            >
                          {/if}
                        {/if}
                      </div>
                    </td>
                  </tr>
                {/if}
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </Card>

    <div class="mt-6">
      <Card>
        <div class="-m-4">
          <LogViewer entries={logs} />
        </div>
      </Card>
    </div>
  {:else if !sseConnected}
    <AlertBanner variant="err" message="Disconnected from forwarder." />
  {:else if !error}
    <p class="text-sm text-text-muted">Loading...</p>
  {/if}
</main>
