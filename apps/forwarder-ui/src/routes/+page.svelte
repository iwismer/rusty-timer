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
    computeElapsedSecondsSince,
    formatLastSeen,
    readerBadgeState,
    readerConnectionSummary,
    formatClockDrift,
    formatReadMode,
    formatTtoState,
    readerControlDisabled,
    computeDownloadPercent,
    computeTickingLastSeen,
  } from "$lib/status-view-model";
  import { pushLogEntry } from "$lib/log-buffer";
  import {
    READ_MODE_OPTIONS,
    initialTimeoutDraft,
    resolveTimeoutSeconds,
    shouldShowTimeoutInput,
  } from "$lib/read-mode-form";
  import {
    subscribeDownloadProgress,
    type DownloadProgressEvent,
    type DownloadProgressHandle,
  } from "$lib/download-progress";
  import {
    applyReaderInfoUpdate,
    clearReaderInfoForIp,
    rebuildReaderCachesFromStatus,
  } from "$lib/reader-status-cache";

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
  let readModeDrafts = $state<Record<string, string>>({});
  let readModeTimeoutDrafts = $state<Record<string, string>>({});
  let controlBusy = $state<Record<string, boolean>>({});
  let controlFeedback = $state<
    Record<string, { kind: "ok" | "err"; message: string } | undefined>
  >({});
  let downloadState = $state<Record<string, DownloadProgressEvent | null>>({});
  let downloadHandles: Record<string, DownloadProgressHandle> = {};
  let localClockStr = $state("");
  let readerInfoReceivedAt = $state<Record<string, number>>({});
  let clockTickNow = $state(Date.now());
  let readerClockBaseTs = $state<Record<string, number>>({});
  let readerClockBaseLocal = $state<Record<string, number>>({});
  let lastSeenBase = $state<Record<string, number | null>>({});
  let lastSeenReceivedAt = $state<Record<string, number>>({});

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
        const now = Date.now();
        const rebuilt = rebuildReaderCachesFromStatus(
          status,
          {
            readerInfoMap,
            readerInfoReceivedAt,
            readerClockBaseTs,
            readerClockBaseLocal,
            lastSeenBase,
            lastSeenReceivedAt,
          },
          now,
        );
        readerInfoMap = rebuilt.readerInfoMap;
        readerInfoReceivedAt = rebuilt.readerInfoReceivedAt;
        readerClockBaseTs = rebuilt.readerClockBaseTs;
        readerClockBaseLocal = rebuilt.readerClockBaseLocal;
        lastSeenBase = rebuilt.lastSeenBase;
        lastSeenReceivedAt = rebuilt.lastSeenReceivedAt;
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

  function readModeDraftValue(
    ip: string,
    info: api.ReaderInfo | undefined,
  ): "raw" | "event" | "fsls" {
    return (
      (readModeDrafts[ip] as "raw" | "event" | "fsls" | undefined) ??
      info?.config?.mode ??
      "raw"
    );
  }

  function readModeTimeoutDraftValue(
    ip: string,
    info: api.ReaderInfo | undefined,
  ) {
    return (
      readModeTimeoutDrafts[ip] ?? initialTimeoutDraft(info?.config?.timeout)
    );
  }

  function updateReadModeDraft(
    ip: string,
    mode: "raw" | "event" | "fsls",
    info: api.ReaderInfo | undefined,
  ) {
    readModeDrafts = { ...readModeDrafts, [ip]: mode };
    if (shouldShowTimeoutInput(mode) && readModeTimeoutDrafts[ip] == null) {
      readModeTimeoutDrafts = {
        ...readModeTimeoutDrafts,
        [ip]: initialTimeoutDraft(info?.config?.timeout),
      };
    }
  }

  function updateReadModeTimeoutDraft(ip: string, value: string) {
    readModeTimeoutDrafts = { ...readModeTimeoutDrafts, [ip]: value };
  }

  function readerDetailsId(ip: string): string {
    return `reader-details-${ip.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
  }

  async function handleSyncClock(ip: string) {
    controlBusy = { ...controlBusy, [ip]: true };
    controlFeedback = { ...controlFeedback, [ip]: undefined };
    try {
      const result = await api.syncReaderClock(ip);
      readerInfoMap = {
        ...readerInfoMap,
        [ip]: {
          ...readerInfoMap[ip],
          clock: {
            reader_clock: result.reader_clock,
            drift_ms: result.clock_drift_ms ?? 0,
          },
        },
      };
      readerInfoReceivedAt = { ...readerInfoReceivedAt, [ip]: Date.now() };
      if (result.reader_clock) storeReaderClockBase(ip, result.reader_clock);
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

  async function handleSetReadMode(
    ip: string,
    mode: "raw" | "event" | "fsls",
    timeoutDraft: string,
    currentTimeout: number | null | undefined,
  ) {
    const timeout = resolveTimeoutSeconds(timeoutDraft, currentTimeout);
    controlBusy = { ...controlBusy, [ip]: true };
    controlFeedback = { ...controlFeedback, [ip]: undefined };
    try {
      const result = await api.setReadMode(ip, mode, timeout);
      readerInfoMap = {
        ...readerInfoMap,
        [ip]: {
          ...readerInfoMap[ip],
          config: {
            mode: result.mode as "raw" | "event" | "fsls",
            timeout,
          },
        },
      };
      readModeDrafts = { ...readModeDrafts, [ip]: result.mode };
      readModeTimeoutDrafts = {
        ...readModeTimeoutDrafts,
        [ip]: String(timeout),
      };
      controlFeedback = {
        ...controlFeedback,
        [ip]: {
          kind: "ok",
          message: shouldShowTimeoutInput(result.mode)
            ? `Mode set to ${formatReadMode(result.mode)} (${timeout}s)`
            : `Mode set to ${formatReadMode(result.mode)}`,
        },
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
      const info = await api.refreshReader(ip);
      if (info) {
        readerInfoMap = {
          ...readerInfoMap,
          [ip]: { ...readerInfoMap[ip], ...info },
        };
        readerInfoReceivedAt = { ...readerInfoReceivedAt, [ip]: Date.now() };
        if (info.clock?.reader_clock)
          storeReaderClockBase(ip, info.clock.reader_clock);
      }
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

  async function handleReconnect(ip: string) {
    controlBusy = { ...controlBusy, [ip]: true };
    controlFeedback = { ...controlFeedback, [ip]: undefined };
    try {
      await api.reconnectReader(ip);
      controlFeedback = {
        ...controlFeedback,
        [ip]: { kind: "ok", message: "Reconnect requested" },
      };
    } catch (e) {
      controlFeedback = {
        ...controlFeedback,
        [ip]: { kind: "err", message: `Reconnect failed: ${e}` },
      };
    } finally {
      controlBusy = { ...controlBusy, [ip]: false };
    }
  }

  async function handleToggleRecording(ip: string) {
    const info = readerInfoMap[ip];
    const currentlyRecording = info?.recording === true;
    controlBusy = { ...controlBusy, [ip]: true };
    controlFeedback = { ...controlFeedback, [ip]: undefined };
    try {
      const result = await api.setRecording(ip, !currentlyRecording);
      readerInfoMap = {
        ...readerInfoMap,
        [ip]: { ...readerInfoMap[ip], recording: result.recording },
      };
      controlFeedback = {
        ...controlFeedback,
        [ip]: {
          kind: "ok",
          message: result.recording ? "Recording started" : "Recording stopped",
        },
      };
    } catch (e) {
      controlFeedback = {
        ...controlFeedback,
        [ip]: { kind: "err", message: `Toggle recording failed: ${e}` },
      };
    } finally {
      controlBusy = { ...controlBusy, [ip]: false };
    }
  }

  async function handleToggleTto(ip: string) {
    const info = readerInfoMap[ip];
    const currentlyEnabled = info?.tto_enabled === true;
    controlBusy = { ...controlBusy, [ip]: true };
    controlFeedback = { ...controlFeedback, [ip]: undefined };
    try {
      const result = await api.setTtoState(ip, !currentlyEnabled);
      readerInfoMap = {
        ...readerInfoMap,
        [ip]: { ...readerInfoMap[ip], tto_enabled: result.enabled },
      };
      controlFeedback = {
        ...controlFeedback,
        [ip]: {
          kind: "ok",
          message: result.enabled
            ? "TTO reporting enabled"
            : "TTO reporting disabled",
        },
      };
    } catch (e) {
      controlFeedback = {
        ...controlFeedback,
        [ip]: { kind: "err", message: `TTO toggle failed: ${e}` },
      };
    } finally {
      controlBusy = { ...controlBusy, [ip]: false };
    }
  }

  async function handleDownloadReads(ip: string) {
    controlBusy = { ...controlBusy, [ip]: true };
    controlFeedback = { ...controlFeedback, [ip]: undefined };
    downloadState = { ...downloadState, [ip]: null };

    try {
      await api.startDownloadReads(ip);

      // Open SSE to track progress
      downloadHandles[ip]?.close();
      downloadHandles[ip] = subscribeDownloadProgress(
        ip,
        (event) => {
          downloadState = { ...downloadState, [ip]: event };
          if (event.state === "complete") {
            controlFeedback = {
              ...controlFeedback,
              [ip]: {
                kind: "ok",
                message: `Download complete: ${event.reads_received} reads received`,
              },
            };
            controlBusy = { ...controlBusy, [ip]: false };
            delete downloadHandles[ip];
          } else if (event.state === "error") {
            controlFeedback = {
              ...controlFeedback,
              [ip]: {
                kind: "err",
                message: `Download failed: ${event.message}`,
              },
            };
            controlBusy = { ...controlBusy, [ip]: false };
            delete downloadHandles[ip];
          }
        },
        () => {
          // SSE connection error
          controlFeedback = {
            ...controlFeedback,
            [ip]: {
              kind: "err",
              message: "Lost connection to download progress",
            },
          };
          controlBusy = { ...controlBusy, [ip]: false };
          downloadState = { ...downloadState, [ip]: null };
          delete downloadHandles[ip];
        },
      );
    } catch (e) {
      controlFeedback = {
        ...controlFeedback,
        [ip]: { kind: "err", message: `Download failed: ${e}` },
      };
      controlBusy = { ...controlBusy, [ip]: false };
    }
  }

  let clockInterval: ReturnType<typeof setInterval>;

  function updateLocalClock() {
    const now = new Date();
    const y = now.getFullYear();
    const mo = String(now.getMonth() + 1).padStart(2, "0");
    const d = String(now.getDate()).padStart(2, "0");
    const h = String(now.getHours()).padStart(2, "0");
    const mi = String(now.getMinutes()).padStart(2, "0");
    const s = String(now.getSeconds()).padStart(2, "0");
    localClockStr = `${y}-${mo}-${d} ${h}:${mi}:${s}`;
    clockTickNow = Date.now();
  }

  function parseReaderClock(iso: string): number {
    // Parse as UTC to avoid timezone ambiguity
    const normalized = iso.replace(" ", "T");
    const withZ = normalized.endsWith("Z") ? normalized : normalized + "Z";
    return new Date(withZ).getTime();
  }

  function storeReaderClockBase(ip: string, clockStr: string) {
    const ts = parseReaderClock(clockStr);
    if (!isNaN(ts)) {
      readerClockBaseTs = { ...readerClockBaseTs, [ip]: ts };
      readerClockBaseLocal = { ...readerClockBaseLocal, [ip]: Date.now() };
    }
  }

  function tickingLastSeen(ip: string): number | null {
    return computeTickingLastSeen(
      lastSeenBase[ip] ?? null,
      lastSeenReceivedAt[ip] ?? null,
      clockTickNow,
    );
  }

  function tickingReaderClock(ip: string): string {
    const baseTs = readerClockBaseTs[ip];
    const baseLocal = readerClockBaseLocal[ip];
    if (baseTs == null || baseLocal == null) return "\u2014";
    const elapsed = clockTickNow - baseLocal;
    const now = new Date(baseTs + elapsed);
    const y = now.getUTCFullYear();
    const mo = String(now.getUTCMonth() + 1).padStart(2, "0");
    const d = String(now.getUTCDate()).padStart(2, "0");
    const h = String(now.getUTCHours()).padStart(2, "0");
    const mi = String(now.getUTCMinutes()).padStart(2, "0");
    const s = String(now.getUTCSeconds()).padStart(2, "0");
    return `${y}-${mo}-${d} ${h}:${mi}:${s}`;
  }

  onMount(() => {
    updateLocalClock();
    clockInterval = setInterval(updateLocalClock, 1000);
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
          lastSeenBase = {
            ...lastSeenBase,
            [reader.ip]: reader.last_seen_secs,
          };
          lastSeenReceivedAt = {
            ...lastSeenReceivedAt,
            [reader.ip]: Date.now(),
          };
          if (reader.state === "disconnected") {
            const cleared = clearReaderInfoForIp(
              {
                readerInfoMap,
                readerInfoReceivedAt,
                readerClockBaseTs,
                readerClockBaseLocal,
                lastSeenBase,
                lastSeenReceivedAt,
              },
              reader.ip,
            );
            readerInfoMap = cleared.readerInfoMap;
            readerInfoReceivedAt = cleared.readerInfoReceivedAt;
            readerClockBaseTs = cleared.readerClockBaseTs;
            readerClockBaseLocal = cleared.readerClockBaseLocal;
          }
        }
      },
      onLogEntry: (entry) => {
        logs = pushLogEntry(logs, entry);
      },
      onReaderInfoUpdated: (data) => {
        const next = applyReaderInfoUpdate(
          status,
          {
            readerInfoMap,
            readerInfoReceivedAt,
            readerClockBaseTs,
            readerClockBaseLocal,
            lastSeenBase,
            lastSeenReceivedAt,
          },
          data,
          Date.now(),
        );
        readerInfoMap = next.readerInfoMap;
        readerInfoReceivedAt = next.readerInfoReceivedAt;
        readerClockBaseTs = next.readerClockBaseTs;
        readerClockBaseLocal = next.readerClockBaseLocal;
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

  onDestroy(() => {
    clearInterval(clockInterval);
    destroySSE();
    for (const handle of Object.values(downloadHandles)) {
      handle.close();
    }
  });
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
        <div class="flex flex-col gap-4">
          {#each status.readers as reader}
            {@const info = readerInfoMap[reader.ip]}
            <Card borderStatus={readerBadgeState(reader.state)}>
              {#snippet header()}
                <span class="font-mono text-sm text-text-primary"
                  >{reader.ip}</span
                >
                <StatusBadge
                  label={reader.state}
                  state={readerBadgeState(reader.state)}
                />
                <div class="ml-auto flex gap-2">
                  {#if reader.state !== "connected"}
                    <button
                      class="px-2 py-1 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50 disabled:cursor-not-allowed"
                      onclick={() => handleReconnect(reader.ip)}
                      disabled={controlBusy[reader.ip]}
                    >
                      Reconnect
                    </button>
                  {/if}
                  <button
                    class="inline-flex items-center gap-1 px-2 py-1 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2"
                    onclick={() => toggleReaderExpand(reader.ip)}
                    aria-expanded={expandedReader === reader.ip}
                    aria-controls={readerDetailsId(reader.ip)}
                    aria-label={expandedReader === reader.ip
                      ? "Hide details"
                      : "Show details"}
                  >
                    <span
                      class={`inline-block transition-transform ${expandedReader === reader.ip ? "rotate-180" : ""}`}
                      >▾</span
                    >
                    <span>Details</span>
                  </button>
                </div>
              {/snippet}

              <!-- Always-visible stats row -->
              <div class="flex flex-wrap gap-x-6 gap-y-2 text-sm mb-3">
                <div>
                  <span class="text-text-muted">Reads (session):</span>
                  <span class="font-mono ml-1 text-text-primary"
                    >{reader.reads_session.toLocaleString()}</span
                  >
                </div>
                <div>
                  <span class="text-text-muted">Reads (total):</span>
                  <span class="font-mono ml-1 text-text-primary"
                    >{reader.reads_total.toLocaleString()}</span
                  >
                </div>
                <div>
                  <span class="text-text-muted">Local Port:</span>
                  <span class="font-mono ml-1 text-text-primary"
                    >{reader.local_port}</span
                  >
                </div>
                <div>
                  <span class="text-text-muted">Last seen:</span>
                  <span class="ml-1 text-text-secondary"
                    >{formatLastSeen(tickingLastSeen(reader.ip))}</span
                  >
                </div>
              </div>

              <!-- Epoch name row -->
              <div class="flex flex-col gap-1">
                {#if reader.current_epoch_name}
                  <span class="text-xs text-text-muted font-mono">
                    Active epoch: {reader.current_epoch_name}
                  </span>
                {/if}
                <div class="flex items-center gap-2 flex-wrap">
                  <input
                    type="text"
                    class="w-48 px-2 py-1 text-xs rounded-md bg-surface-0 text-text-primary border border-border"
                    placeholder="Set epoch name"
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
                  <button
                    onclick={() => handleResetEpoch(reader.ip)}
                    class="px-2 py-1 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2"
                  >
                    Advance Epoch
                  </button>
                </div>
                {#if epochNameFeedback[reader.ip]}
                  {@const feedback = epochNameFeedback[reader.ip]}
                  {#if feedback}
                    <span
                      class={`text-xs ${feedback.kind === "ok" ? "text-status-ok" : "text-status-err"}`}
                    >
                      {feedback.message}
                    </span>
                  {/if}
                {/if}
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

              <!-- Expanded details -->
              {#if expandedReader === reader.ip}
                <div
                  id={readerDetailsId(reader.ip)}
                  class="mt-4 pt-4 border-t border-border"
                >
                  <div class="grid grid-cols-2 gap-x-8 gap-y-2 text-sm mb-4">
                    <div class="col-span-2">
                      <span class="text-text-muted">Banner:</span>
                      <span class="font-mono ml-2 text-xs"
                        >{info?.banner ?? "\u2014"}</span
                      >
                    </div>
                    <div>
                      <span class="text-text-muted">Firmware:</span>
                      <span class="font-mono ml-2"
                        >{info?.hardware?.fw_version ?? "\u2014"}</span
                      >
                    </div>
                    <div>
                      <span class="text-text-muted">Hardware:</span>
                      <span class="font-mono ml-2"
                        >{info?.hardware?.hw_code != null
                          ? `0x${info.hardware.hw_code.toString(16)}`
                          : "\u2014"}</span
                      >
                    </div>
                    <div>
                      <span class="text-text-muted">Reader Clock:</span>
                      <span class="font-mono ml-2"
                        >{tickingReaderClock(reader.ip)}</span
                      >
                    </div>
                    <div>
                      <span class="text-text-muted">Clock Drift:</span>
                      <span class="font-mono ml-2"
                        >{formatClockDrift(info?.clock?.drift_ms)}</span
                      >
                    </div>
                    <div>
                      <span class="text-text-muted">Local Clock:</span>
                      <span class="font-mono ml-2">{localClockStr}</span>
                    </div>
                    <div>
                      <span class="text-text-muted">Last Refresh:</span>
                      <span class="ml-2"
                        >{#if readerInfoReceivedAt[reader.ip]}{formatLastSeen(
                            computeElapsedSecondsSince(
                              readerInfoReceivedAt[reader.ip],
                              clockTickNow,
                            ),
                          )}{:else}&mdash;{/if}</span
                      >
                    </div>
                    <div class="col-span-2">
                      <span class="text-text-muted">Read Mode:</span>
                      <span
                        class="ml-2 inline-flex items-center gap-2 flex-wrap"
                      >
                        <select
                          class="px-2 py-0.5 text-sm rounded-md bg-surface-0 text-text-primary border border-border"
                          value={readModeDraftValue(reader.ip, info)}
                          onchange={(e) => {
                            e.stopPropagation();
                            updateReadModeDraft(
                              reader.ip,
                              (e.currentTarget as HTMLSelectElement).value as
                                | "raw"
                                | "event"
                                | "fsls",
                              info,
                            );
                          }}
                          disabled={controlBusy[reader.ip] ||
                            reader.state !== "connected"}
                        >
                          {#each READ_MODE_OPTIONS as option}
                            <option value={option.value}>{option.label}</option>
                          {/each}
                        </select>
                        {#if shouldShowTimeoutInput(readModeDraftValue(reader.ip, info))}
                          <label
                            class="inline-flex items-center gap-1 text-xs text-text-secondary"
                          >
                            <span>Timeout</span>
                            <input
                              class="w-16 px-2 py-0.5 text-sm rounded-md bg-surface-0 text-text-primary border border-border"
                              type="number"
                              min="1"
                              max="255"
                              value={readModeTimeoutDraftValue(reader.ip, info)}
                              oninput={(e) => {
                                e.stopPropagation();
                                updateReadModeTimeoutDraft(
                                  reader.ip,
                                  (e.currentTarget as HTMLInputElement).value,
                                );
                              }}
                              disabled={controlBusy[reader.ip] ||
                                reader.state !== "connected"}
                            />
                            <span>s</span>
                          </label>
                        {/if}
                        <button
                          class="px-2.5 py-0.5 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
                          onclick={(e) => {
                            e.stopPropagation();
                            handleSetReadMode(
                              reader.ip,
                              readModeDraftValue(reader.ip, info),
                              readModeTimeoutDraftValue(reader.ip, info),
                              info?.config?.timeout,
                            );
                          }}
                          disabled={controlBusy[reader.ip] ||
                            reader.state !== "connected"}>Apply</button
                        >
                      </span>
                    </div>
                    <div class="col-span-2">
                      <span class="text-text-muted">TTO Bytes:</span>
                      <span
                        class="ml-2 inline-flex items-center gap-2 flex-wrap"
                      >
                        <span class="font-mono"
                          >{formatTtoState(info?.tto_enabled)}</span
                        >
                        <button
                          class="px-2.5 py-0.5 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
                          onclick={(e) => {
                            e.stopPropagation();
                            handleToggleTto(reader.ip);
                          }}
                          disabled={readerControlDisabled(
                            reader.state,
                            controlBusy[reader.ip],
                          )}
                        >
                          {info?.tto_enabled ? "Disable TTO" : "Enable TTO"}
                        </button>
                      </span>
                    </div>
                  </div>
                  <div
                    class="flex items-center gap-3 pt-3 border-t border-border flex-wrap"
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
                      class={info?.recording
                        ? "px-3 py-1.5 text-sm rounded-md bg-red-600 text-white border-none cursor-pointer hover:bg-red-700 disabled:opacity-50"
                        : "px-3 py-1.5 text-sm rounded-md bg-green-600 text-white border-none cursor-pointer hover:bg-green-700 disabled:opacity-50"}
                      onclick={(e) => {
                        e.stopPropagation();
                        handleToggleRecording(reader.ip);
                      }}
                      disabled={controlBusy[reader.ip] ||
                        reader.state !== "connected"}
                      >{info?.recording
                        ? "Stop Recording"
                        : "Start Recording"}</button
                    >
                    <button
                      class={btnPrimary}
                      onclick={(e) => {
                        e.stopPropagation();
                        handleDownloadReads(reader.ip);
                      }}
                      disabled={controlBusy[reader.ip] ||
                        reader.state !== "connected"}>Download Reads</button
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
                  </div>
                  {#if downloadState[reader.ip]?.state === "downloading"}
                    {@const dl = downloadState[reader.ip]}
                    {@const percent = computeDownloadPercent(
                      dl,
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
                        {dl?.state === "downloading" || dl?.state === "complete"
                          ? dl.reads_received
                          : 0} reads &middot; {percent}%
                      </span>
                    </div>
                  {/if}
                  {#if controlFeedback[reader.ip]}
                    {@const fb = controlFeedback[reader.ip]}
                    {#if fb}
                      <div class="mt-3">
                        <AlertBanner
                          variant={fb.kind}
                          message={fb.message}
                          onDismiss={() => {
                            controlFeedback = {
                              ...controlFeedback,
                              [reader.ip]: undefined,
                            };
                          }}
                        />
                      </div>
                    {/if}
                  {/if}
                </div>
              {/if}
            </Card>
          {/each}
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
