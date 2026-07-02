<script lang="ts">
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { onMount, onDestroy, untrack } from "svelte";
  import {
    BatteryIndicator,
    ReaderControlPanel,
    computeTickingClock,
    formatLastSeen,
    parseWallClock,
  } from "@rusty-timer/shared-ui";
  import { loadConnections, store } from "$lib/store.svelte";
  import type {
    ForwarderConnectionStatus,
    ReaderControlResult,
    ReaderLiveStatus,
    ServerDeviceStatus,
  } from "$lib/api";
  import {
    connectForwarder,
    disconnectForwarder,
    readerAdvanceEpoch,
    readerClearRecords,
    readerReconnect,
    readerRefresh,
    readerSetEpochName,
    readerSetReadMode,
    readerSetRecording,
    readerSetTto,
    readerStartDownload,
    readerStopDownload,
    readerSyncClock,
    reconnectForwarder,
  } from "$lib/api";
  import { btnPrimary, btnSecondary } from "$lib/ui-classes";
  import ForwarderConfigModal from "./ForwarderConfigModal.svelte";

  type StateDisplay = {
    label: string;
    dotClass: string;
    textClass: string;
  };

  let busyByEndpoint = $state<Record<string, boolean>>({});
  let actionError = $state<string | null>(null);
  let configEndpointId = $state<string | null>(null);

  // Reader/forwarder clock display. The reader clock arrives on status refresh;
  // we anchor it (baseTs) to the wall time it was captured (baseLocal) and tick
  // it forward locally so the display advances smoothly between refreshes, the
  // same way the forwarder UI does. The forwarder-host clock is derived from the
  // reader clock plus the measured reader->forwarder drift so both render in the
  // same wall-clock frame and line up when in sync.
  let clockNow = $state(Date.now());
  let readerClockBaseTs = $state<Record<string, number>>({});
  let readerClockBaseLocal = $state<Record<string, number>>({});
  const readerClockLastStr: Record<string, string> = {};
  let clockInterval: ReturnType<typeof setInterval> | undefined;

  onMount(() => {
    clockInterval = setInterval(() => {
      clockNow = Date.now();
    }, 1000);
  });

  onDestroy(() => {
    if (clockInterval !== undefined) clearInterval(clockInterval);
  });

  // Re-anchor a reader's clock base whenever its reported reader_clock changes.
  $effect(() => {
    const connections = store.connections;
    if (!connections) return;
    for (const forwarder of connections.forwarders) {
      for (const reader of forwarder.readers) {
        const clockStr = reader.reader_info?.clock?.reader_clock;
        if (!clockStr) continue;
        const key = reader.stream_id;
        if (readerClockLastStr[key] === clockStr) continue;
        const ts = parseWallClock(clockStr);
        if (Number.isNaN(ts)) continue;
        readerClockLastStr[key] = clockStr;
        untrack(() => {
          readerClockBaseTs = { ...readerClockBaseTs, [key]: ts };
          readerClockBaseLocal = { ...readerClockBaseLocal, [key]: Date.now() };
        });
      }
    }
  });

  function readerClockDisplay(reader: ReaderLiveStatus): string | undefined {
    const key = reader.stream_id;
    return (
      computeTickingClock(
        readerClockBaseTs[key],
        readerClockBaseLocal[key],
        clockNow,
      ) ?? undefined
    );
  }

  function forwarderClockDisplay(reader: ReaderLiveStatus): string | undefined {
    const driftMs = reader.reader_info?.clock?.drift_ms;
    if (driftMs == null) return undefined;
    const key = reader.stream_id;
    return (
      computeTickingClock(
        readerClockBaseTs[key],
        readerClockBaseLocal[key],
        clockNow,
        driftMs,
      ) ?? undefined
    );
  }

  function approvalLabel(server: ServerDeviceStatus): string | null {
    if (!server.configured) return "Server not configured";
    if (server.waiting_for_approval) {
      return server.message ?? "Waiting for server approval";
    }
    if (server.reachable === false)
      return server.message ?? "Server unreachable";
    if (server.approval_state === "active") return "Server approved";
    return server.message;
  }

  function approvalClass(server: ServerDeviceStatus): string {
    if (server.waiting_for_approval) return "text-status-warn";
    if (server.reachable === false) return "text-status-err";
    return "text-text-muted";
  }

  function reachableLabel(server: ServerDeviceStatus): string {
    if (!server.configured) return "Not configured";
    if (server.reachable === true) return "Reachable";
    if (server.reachable === false) return "Unreachable";
    return "Reachability unknown";
  }

  function forwarderStateDisplay(
    forwarder: ForwarderConnectionStatus,
  ): StateDisplay {
    if (forwarder.pending) {
      return {
        label: "Connecting…",
        dotClass: "bg-status-warn",
        textClass: "text-status-warn",
      };
    }

    switch (forwarder.state) {
      case "subscribed":
        return {
          label: "Subscribed",
          dotClass: "bg-status-ok",
          textClass: "text-status-ok",
        };
      case "connected":
        return {
          label: "Connected",
          dotClass: "bg-status-ok",
          textClass: "text-status-ok",
        };
      case "unavailable":
        return {
          label: "Unavailable",
          dotClass: "bg-status-err",
          textClass: "text-status-err",
        };
      case "disconnected":
        return {
          label: "Disconnected",
          dotClass: "bg-text-muted",
          textClass: "text-text-muted",
        };
      default:
        return {
          label: "Unknown",
          dotClass: "bg-text-muted",
          textClass: "text-text-muted",
        };
    }
  }

  function adminUrl(): string | null {
    const base = store.savedServerUrl.trim().replace(/\/+$/, "");
    if (base.length === 0) return null;
    return `${base}/admin`;
  }

  async function openAdminPanel(): Promise<void> {
    const url = adminUrl();
    if (!url) return;
    await openUrl(url);
  }

  async function runForwarderAction(
    endpointId: string,
    action: (endpointId: string) => Promise<void>,
  ): Promise<void> {
    busyByEndpoint[endpointId] = true;
    actionError = null;
    try {
      await action(endpointId);
    } catch (e) {
      actionError = String(e);
    } finally {
      busyByEndpoint[endpointId] = false;
    }
  }

  function showConnect(forwarder: ForwarderConnectionStatus): boolean {
    return !forwarder.pending && forwarder.state === "disconnected";
  }

  function showDisconnect(forwarder: ForwarderConnectionStatus): boolean {
    return forwarder.pending || forwarder.state !== "disconnected";
  }

  function showReconnect(forwarder: ForwarderConnectionStatus): boolean {
    return forwarder.pending || forwarder.state !== "disconnected";
  }

  function showReconnectBeforeDisconnect(
    forwarder: ForwarderConnectionStatus,
  ): boolean {
    return !forwarder.pending && forwarder.state === "unavailable";
  }

  function readerLabel(reader: ReaderLiveStatus): string {
    return reader.stream_id;
  }

  function readerForwarderStateLabel(reader: ReaderLiveStatus): string {
    if (reader.connected) return "connected to forwarder";
    if (reader.state === "connecting") return "connecting to forwarder";
    return "disconnected from forwarder";
  }

  function readerConnectionState(
    reader: ReaderLiveStatus,
  ): "connected" | "connecting" | "disconnected" {
    if (reader.connected) return "connected";
    if (reader.state === "connecting") return "connecting";
    return "disconnected";
  }

  function readerInfoForPanel(reader: ReaderLiveStatus) {
    if (reader.reader_info) return reader.reader_info;
    if (
      !reader.hardware_reader_id &&
      !reader.firmware_version &&
      !reader.model
    ) {
      return null;
    }
    return {
      hardware: {
        reader_id: reader.hardware_reader_id,
        fw_version: reader.firmware_version,
        hw_code: reader.model,
      },
    };
  }

  function localPortValueForPanel(reader: ReaderLiveStatus): string {
    return reader.local_port == null
      ? "not subscribed"
      : `127.0.0.1:${reader.local_port}`;
  }

  function lastSeenDisplayForPanel(
    reader: ReaderLiveStatus,
  ): string | undefined {
    if (reader.last_seen_secs === undefined) return undefined;
    return formatLastSeen(reader.last_seen_secs);
  }

  function downloadProgressForPanel(reader: ReaderLiveStatus) {
    const progress = reader.download_progress;
    if (!progress) return null;
    return {
      state: progress.state,
      reads_received: progress.downloaded_reads,
      progress: progress.progress,
      total: progress.total ?? 0,
      error: progress.error ?? undefined,
    };
  }

  async function runReaderCommand(
    command: () => Promise<ReaderControlResult>,
  ): Promise<void> {
    const result = await command();
    await loadConnections();
    if (!result.success) {
      throw new Error(result.message || "Reader command failed");
    }
  }

  function selectedConfigForwarder(): ForwarderConnectionStatus | null {
    if (!configEndpointId || !store.connections) return null;
    return (
      store.connections.forwarders.find(
        (forwarder) => forwarder.endpoint_id === configEndpointId,
      ) ?? null
    );
  }
</script>

<div class="mx-auto max-w-[760px] px-6 py-6">
  {#if store.connections}
    <section
      data-testid="connections-server-card"
      class="rounded-lg border border-border bg-surface-1 p-4"
    >
      <div class="flex items-start justify-between gap-4">
        <div>
          <p class="text-xs font-medium text-text-muted">Server</p>
          <p class="mt-1 text-sm text-text-primary">
            {reachableLabel(store.connections.server)}
          </p>
          {#if store.connections.server.endpoint_id}
            <p class="mt-1 font-mono text-xs text-text-muted">
              {store.connections.server.endpoint_id}
            </p>
          {/if}
          {#if approvalLabel(store.connections.server)}
            <p
              data-testid="server-approval-state"
              class="mt-2 text-xs {approvalClass(store.connections.server)}"
            >
              {approvalLabel(store.connections.server)}
            </p>
          {/if}
        </div>

        {#if store.connections.server.configured}
          <button
            data-testid="open-admin-panel-btn"
            class={btnSecondary}
            onclick={() => void openAdminPanel()}
            disabled={!adminUrl()}
          >
            Open admin panel
          </button>
        {/if}
      </div>
    </section>

    <section class="mt-4 rounded-lg border border-border bg-surface-1">
      <div class="border-b border-border px-4 py-3">
        <p class="text-xs font-medium text-text-muted">Forwarders</p>
      </div>

      {#if store.connections.forwarders.length === 0}
        <p class="px-4 py-6 text-sm text-text-muted">
          No forwarders are available yet.
        </p>
      {:else}
        <div class="divide-y divide-border">
          {#each store.connections.forwarders as forwarder (forwarder.endpoint_id)}
            {@const stateDisplay = forwarderStateDisplay(forwarder)}
            <div
              data-testid={`forwarder-row-${forwarder.endpoint_id}`}
              class="px-4 py-3"
            >
              <div class="min-w-0">
                <div class="flex items-center gap-2">
                  <span
                    data-testid={`forwarder-state-${forwarder.endpoint_id}`}
                    class="flex items-center gap-2 text-sm font-medium {stateDisplay.textClass}"
                  >
                    <span class="h-2 w-2 rounded-full {stateDisplay.dotClass}"
                    ></span>
                    {stateDisplay.label}
                  </span>
                  <span class="text-xs text-text-muted">
                    {forwarder.subscribed_count} subscribed / {forwarder.available_count}
                    available
                  </span>
                </div>
                <p class="mt-1 truncate text-sm font-medium text-text-primary">
                  {forwarder.display_name ?? forwarder.endpoint_id}
                </p>
                <p class="mt-0.5 truncate font-mono text-xs text-text-muted">
                  {forwarder.endpoint_id}
                </p>
                {#if forwarder.ups}
                  <div class="mt-2 flex flex-wrap items-center gap-2">
                    <span
                      data-testid={`forwarder-ups-${forwarder.endpoint_id}`}
                      class="inline-flex items-center rounded-full border border-border bg-surface-2 px-2 py-0.5 text-xs"
                    >
                      <BatteryIndicator
                        percent={forwarder.ups.battery_percent}
                        charging={!forwarder.ups.on_battery}
                      />
                    </span>
                  </div>
                {/if}

                {#if forwarder.remote_config_available === true || showConnect(forwarder) || showReconnectBeforeDisconnect(forwarder) || showDisconnect(forwarder) || (showReconnect(forwarder) && !showReconnectBeforeDisconnect(forwarder))}
                  <div class="mt-3 flex flex-wrap items-center gap-2">
                    {#if forwarder.remote_config_available === true}
                      <button
                        data-testid={`forwarder-configure-${forwarder.endpoint_id}`}
                        class={btnSecondary}
                        onclick={() =>
                          (configEndpointId = forwarder.endpoint_id)}
                      >
                        Configure
                      </button>
                    {/if}
                    {#if showConnect(forwarder)}
                      <button
                        data-testid={`forwarder-connect-${forwarder.endpoint_id}`}
                        class={btnPrimary}
                        onclick={() =>
                          void runForwarderAction(
                            forwarder.endpoint_id,
                            connectForwarder,
                          )}
                        disabled={busyByEndpoint[forwarder.endpoint_id]}
                      >
                        Connect
                      </button>
                    {/if}
                    {#if showReconnectBeforeDisconnect(forwarder)}
                      <button
                        data-testid={`forwarder-reconnect-${forwarder.endpoint_id}`}
                        class={btnSecondary}
                        onclick={() =>
                          void runForwarderAction(
                            forwarder.endpoint_id,
                            reconnectForwarder,
                          )}
                        disabled={busyByEndpoint[forwarder.endpoint_id]}
                      >
                        Reconnect
                      </button>
                    {/if}
                    {#if showDisconnect(forwarder)}
                      <button
                        data-testid={`forwarder-disconnect-${forwarder.endpoint_id}`}
                        class={btnSecondary}
                        onclick={() =>
                          void runForwarderAction(
                            forwarder.endpoint_id,
                            disconnectForwarder,
                          )}
                        disabled={busyByEndpoint[forwarder.endpoint_id]}
                      >
                        Disconnect
                      </button>
                    {/if}
                    {#if showReconnect(forwarder) && !showReconnectBeforeDisconnect(forwarder)}
                      <button
                        data-testid={`forwarder-reconnect-${forwarder.endpoint_id}`}
                        class={btnSecondary}
                        onclick={() =>
                          void runForwarderAction(
                            forwarder.endpoint_id,
                            reconnectForwarder,
                          )}
                        disabled={busyByEndpoint[forwarder.endpoint_id]}
                      >
                        Reconnect
                      </button>
                    {/if}
                  </div>
                {/if}

                {#if forwarder.reader_control_available && forwarder.readers.length > 0}
                  <div class="mt-3 space-y-3">
                    {#each forwarder.readers as reader (reader.stream_id)}
                      <ReaderControlPanel
                        readerIp={readerLabel(reader)}
                        readerInfo={readerInfoForPanel(reader)}
                        readerClockDisplay={readerClockDisplay(reader)}
                        forwarderClockDisplay={forwarderClockDisplay(reader)}
                        readerState={readerConnectionState(reader)}
                        readerStateLabel={readerForwarderStateLabel(reader)}
                        readsSession={reader.reads_session ?? null}
                        readsTotal={reader.reads_total ?? null}
                        lastSeenDisplay={lastSeenDisplayForPanel(reader)}
                        currentEpochName={reader.current_epoch_name ?? null}
                        localPortLabel="Local proxy"
                        localPortValue={localPortValueForPanel(reader)}
                        detailsCollapsible
                        defaultCollapsed
                        epochEditable
                        downloadProgress={downloadProgressForPanel(reader)}
                        disabled={busyByEndpoint[forwarder.endpoint_id]}
                        helpContext="forwarder"
                        onSetEpochName={(name) =>
                          runReaderCommand(() =>
                            readerSetEpochName(
                              forwarder.endpoint_id,
                              reader.stream_id,
                              name,
                            ),
                          )}
                        onAdvanceEpoch={() =>
                          runReaderCommand(() =>
                            readerAdvanceEpoch(
                              forwarder.endpoint_id,
                              reader.stream_id,
                            ),
                          )}
                        onSyncClock={() =>
                          runReaderCommand(() =>
                            readerSyncClock(
                              forwarder.endpoint_id,
                              reader.stream_id,
                            ),
                          )}
                        onSetReadMode={(mode, timeout) =>
                          runReaderCommand(() =>
                            readerSetReadMode(
                              forwarder.endpoint_id,
                              reader.stream_id,
                              mode as "raw" | "event" | "fsls",
                              timeout,
                            ),
                          )}
                        onSetTto={(enabled) =>
                          runReaderCommand(() =>
                            readerSetTto(
                              forwarder.endpoint_id,
                              reader.stream_id,
                              enabled,
                            ),
                          )}
                        onSetRecording={(enabled) =>
                          runReaderCommand(() =>
                            readerSetRecording(
                              forwarder.endpoint_id,
                              reader.stream_id,
                              enabled,
                            ),
                          )}
                        onClearRecords={() =>
                          runReaderCommand(() =>
                            readerClearRecords(
                              forwarder.endpoint_id,
                              reader.stream_id,
                            ),
                          )}
                        onStartDownload={() =>
                          runReaderCommand(() =>
                            readerStartDownload(
                              forwarder.endpoint_id,
                              reader.stream_id,
                            ),
                          )}
                        onStopDownload={() =>
                          runReaderCommand(() =>
                            readerStopDownload(
                              forwarder.endpoint_id,
                              reader.stream_id,
                            ),
                          )}
                        onRefresh={() =>
                          runReaderCommand(() =>
                            readerRefresh(
                              forwarder.endpoint_id,
                              reader.stream_id,
                            ),
                          )}
                        onReconnect={() =>
                          runReaderCommand(() =>
                            readerReconnect(
                              forwarder.endpoint_id,
                              reader.stream_id,
                            ),
                          )}
                      />
                    {/each}
                  </div>
                {/if}
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </section>

    {#if actionError}
      <p class="mt-3 text-xs text-status-err">{actionError}</p>
    {/if}

    {@const configForwarder = selectedConfigForwarder()}
    <ForwarderConfigModal
      open={configEndpointId !== null}
      endpointId={configEndpointId}
      displayName={configForwarder?.display_name ?? null}
      onClose={() => (configEndpointId = null)}
    />
  {:else}
    <section
      data-testid="connections-server-card"
      class="rounded-lg border border-border bg-surface-1 p-4"
    >
      <p class="text-sm text-text-muted">Loading connections…</p>
    </section>
  {/if}
</div>
