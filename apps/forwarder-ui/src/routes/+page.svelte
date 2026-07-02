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
    HelpDialog,
    BatteryIndicator,
    ReaderControlPanel,
  } from "@rusty-timer/shared-ui";
  import type { ForwarderStatus } from "$lib/api";
  import {
    computeElapsedSecondsSince,
    formatLastSeen,
    readerBadgeState,
    readerConnectionSummary,
    computeTickingLastSeen,
    computeTickingClock,
  } from "$lib/status-view-model";
  import { pushLogEntry } from "$lib/log-buffer";
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
  let readModeHelpOpen = $state(false);
  let readModeHelpField = $state<string | undefined>(undefined);
  let readerLiveHelpOpen = $state(false);
  let readerLiveHelpField = $state<string | undefined>(undefined);
  let updateVersion = $state<string | null>(null);
  let updateStatus = $state<"available" | "downloaded" | null>(null);
  let updateBusy = $state(false);
  let sseConnected = $state(false);
  let logs = $state<string[]>([]);
  let readerInfoMap = $state<Record<string, api.ReaderInfo>>({});
  let controlBusy = $state<Record<string, boolean>>({});
  let controlFeedback = $state<
    Record<string, { kind: "ok" | "err"; message: string } | undefined>
  >({});
  let downloadState = $state<Record<string, DownloadProgressEvent | null>>({});
  let upsState = $state<{
    available: boolean;
    status: any | null;
  } | null>(null);
  let downloadHandles: Record<string, DownloadProgressHandle> = {};
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

  function serverApprovalLabel(): string | null {
    const thin = status?.server;
    if (!thin?.configured) return null;
    if (thin.waiting_for_approval)
      return thin.message ?? "Waiting for server approval";
    if (thin.reachable === false) return thin.message ?? "Server unreachable";
    if (thin.approval_state === "active") return "Approved by server";
    return thin.message;
  }

  async function loadAll() {
    error = null;
    try {
      status = await api.getStatus();
      if (status) {
        upsState = status.ups_status ?? null;
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

  // Reader control handlers. These throw on failure so the shared
  // ReaderControlPanel can surface the error via its own feedback banner;
  // success feedback is likewise rendered by the panel.
  async function handleResetEpoch(readerIp: string) {
    await api.resetEpoch(readerIp);
  }

  async function handleSetCurrentEpochName(
    readerIp: string,
    name: string | null,
  ) {
    await api.setCurrentEpochName(readerIp, name);
  }

  async function handleSyncClock(ip: string) {
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
  }

  async function handleSetReadMode(ip: string, mode: string, timeout: number) {
    const result = await api.setReadMode(
      ip,
      mode as "raw" | "event" | "fsls",
      timeout,
    );
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
  }

  async function handleRefreshReader(ip: string) {
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
  }

  async function handleClearRecords(ip: string) {
    await api.clearReaderRecords(ip);
  }

  async function handleReconnect(ip: string) {
    await api.reconnectReader(ip);
  }

  async function handleToggleRecording(ip: string, enabled: boolean) {
    const result = await api.setRecording(ip, enabled);
    readerInfoMap = {
      ...readerInfoMap,
      [ip]: { ...readerInfoMap[ip], recording: result.recording },
    };
  }

  async function handleToggleTto(ip: string, enabled: boolean) {
    const result = await api.setTtoState(ip, enabled);
    readerInfoMap = {
      ...readerInfoMap,
      [ip]: { ...readerInfoMap[ip], tto_enabled: result.enabled },
    };
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
      controlBusy = { ...controlBusy, [ip]: false };
      // Re-throw so the panel reports the start failure in its feedback.
      throw e;
    }
  }

  function downloadProgressForPanel(ip: string): {
    state: "downloading" | "complete" | "error" | "idle";
    reads_received: number;
    progress: number;
    total: number;
    error?: string;
  } | null {
    const dl = downloadState[ip];
    if (!dl) return null;
    if (dl.state === "downloading") {
      return {
        state: dl.state,
        reads_received: dl.reads_received,
        progress: dl.progress,
        total: dl.total,
      };
    }
    if (dl.state === "complete") {
      return {
        state: dl.state,
        reads_received: dl.reads_received,
        progress: 0,
        total: 0,
      };
    }
    if (dl.state === "error") {
      return {
        state: dl.state,
        reads_received: 0,
        progress: 0,
        total: 0,
        error: dl.message,
      };
    }
    return { state: dl.state, reads_received: 0, progress: 0, total: 0 };
  }

  function openReaderHelp(fieldKey: string) {
    if (fieldKey === "read_mode" || fieldKey === "timeout") {
      readModeHelpField = fieldKey;
      readModeHelpOpen = true;
    } else {
      readerLiveHelpField = fieldKey;
      readerLiveHelpOpen = true;
    }
  }

  let clockInterval: ReturnType<typeof setInterval>;

  function updateLocalClock() {
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
    return (
      computeTickingClock(
        readerClockBaseTs[ip],
        readerClockBaseLocal[ip],
        clockTickNow,
      ) ?? "\u2014"
    );
  }

  // Forwarder host clock, derived from the reader clock plus the measured
  // reader→forwarder drift (drift = forwarder_local − reader). Rendered in the
  // same wall-clock frame as the reader clock so the two line up when synced,
  // instead of comparing the reader clock against the viewing browser's zone.
  function tickingForwarderClock(ip: string): string {
    const driftMs = readerInfoMap[ip]?.clock?.drift_ms;
    if (driftMs == null) return "\u2014";
    return (
      computeTickingClock(
        readerClockBaseTs[ip],
        readerClockBaseLocal[ip],
        clockTickNow,
        driftMs,
      ) ?? "\u2014"
    );
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
            p2p_connected: data.p2p_connected,
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
          upsState = null;
        }
      },
      onUpsStatusChanged: (payload) => {
        upsState = { available: payload.available, status: payload.status };
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

  {#if upsState}
    {#if upsState.status && !upsState.status.power_plugged}
      <div class="mb-4">
        <AlertBanner
          variant="warn"
          message="Running on battery power ({upsState.status
            .battery_percent}%)"
        />
      </div>
    {/if}
    {#if !upsState.available}
      <div class="mb-4">
        <AlertBanner variant="warn" message="UPS unavailable" />
      </div>
    {/if}
  {/if}

  <h1 class="text-xl font-bold text-text-primary mb-6">Forwarder</h1>

  {#if status}
    <div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-6">
      <Card
        title="Status"
        helpSection="status_overview"
        helpContext="forwarder"
      >
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
      <Card
        title="Service"
        helpSection="service_overview"
        helpContext="forwarder"
      >
        <dl class="grid gap-2 text-sm" style="grid-template-columns: auto 1fr;">
          <dt class="text-text-muted">P2P</dt>
          <dd>
            <StatusBadge
              label={status.p2p_connected ? "connected" : "disconnected"}
              state={status.p2p_connected ? "ok" : "err"}
            />
          </dd>
          {#if serverApprovalLabel()}
            <dt class="text-text-muted">Server</dt>
            <dd>
              <span
                data-testid="server-approval-state"
                class="text-xs {status.server.waiting_for_approval
                  ? 'text-status-warn'
                  : status.server.reachable === false
                    ? 'text-status-err'
                    : 'text-text-muted'}"
              >
                {serverApprovalLabel()}
              </span>
            </dd>
          {/if}
          <dt class="text-text-muted">Restart Needed</dt>
          <dd>
            <StatusBadge
              label={status.restart_needed ? "pending" : "none"}
              state={status.restart_needed ? "warn" : "ok"}
            />
          </dd>
          {#if upsState}
            <dt class="text-text-muted">Battery</dt>
            <dd>
              <BatteryIndicator
                percent={upsState.status?.battery_percent ?? null}
                charging={upsState.status?.charging ?? false}
                available={upsState.available}
                configured
              />
            </dd>
          {/if}
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

    <Card headerBg helpSection="readers" helpContext="forwarder">
      {#snippet header()}
        <h2 class="text-sm font-semibold text-text-primary m-0">Readers</h2>
        <span class="ml-auto text-xs text-text-muted mr-1">
          {readersSummary.label}
        </span>
      {/snippet}

      {#if status.readers.length === 0}
        <p class="text-sm text-text-muted m-0">No readers configured.</p>
      {:else}
        <div class="flex flex-col gap-4">
          {#each status.readers as reader (reader.ip)}
            <Card borderStatus={readerBadgeState(reader.state)}>
              {#snippet header()}
                <span class="font-mono text-sm text-text-primary"
                  >{reader.ip}</span
                >
                <StatusBadge
                  label={reader.state}
                  state={readerBadgeState(reader.state)}
                />
                {#if reader.state !== "connected"}
                  <div class="ml-auto flex gap-2">
                    <button
                      class="px-2 py-1 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50 disabled:cursor-not-allowed"
                      onclick={() => {
                        handleReconnect(reader.ip).catch((e) => {
                          error = String(e);
                        });
                      }}
                      disabled={controlBusy[reader.ip]}
                    >
                      Reconnect
                    </button>
                  </div>
                {/if}
              {/snippet}

              <ReaderControlPanel
                readerIp={reader.ip}
                showHeader={false}
                readerInfo={readerInfoMap[reader.ip] ?? null}
                readerState={reader.state}
                readsSession={reader.reads_session}
                readsTotal={reader.reads_total}
                localPortLabel="Local Port"
                localPortValue={String(reader.local_port)}
                lastSeenDisplay={formatLastSeen(tickingLastSeen(reader.ip))}
                currentEpochName={reader.current_epoch_name ?? null}
                readerClockDisplay={tickingReaderClock(reader.ip)}
                forwarderClockDisplay={tickingForwarderClock(reader.ip)}
                lastRefreshDisplay={readerInfoReceivedAt[reader.ip]
                  ? formatLastSeen(
                      computeElapsedSecondsSince(
                        readerInfoReceivedAt[reader.ip],
                        clockTickNow,
                      ),
                    )
                  : "\u2014"}
                downloadProgress={downloadProgressForPanel(reader.ip)}
                disabled={controlBusy[reader.ip] === true}
                detailsCollapsible
                defaultCollapsed
                helpContext="forwarder"
                onOpenHelpModal={openReaderHelp}
                onSetEpochName={(name) =>
                  handleSetCurrentEpochName(reader.ip, name)}
                onAdvanceEpoch={() => handleResetEpoch(reader.ip)}
                onSyncClock={() => handleSyncClock(reader.ip)}
                onSetReadMode={(mode, timeout) =>
                  handleSetReadMode(reader.ip, mode, timeout)}
                onSetTto={(enabled) => handleToggleTto(reader.ip, enabled)}
                onSetRecording={(enabled) =>
                  handleToggleRecording(reader.ip, enabled)}
                onClearRecords={() => handleClearRecords(reader.ip)}
                onStartDownload={() => handleDownloadReads(reader.ip)}
                onRefresh={() => handleRefreshReader(reader.ip)}
                onReconnect={() => handleReconnect(reader.ip)}
              />

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

<HelpDialog
  open={readModeHelpOpen}
  sectionKey="read_mode"
  context="forwarder"
  scrollToField={readModeHelpField}
  onClose={() => {
    readModeHelpOpen = false;
    readModeHelpField = undefined;
  }}
/>

<HelpDialog
  open={readerLiveHelpOpen}
  sectionKey="reader_live"
  context="forwarder"
  scrollToField={readerLiveHelpField}
  onClose={() => {
    readerLiveHelpOpen = false;
    readerLiveHelpField = undefined;
  }}
/>
