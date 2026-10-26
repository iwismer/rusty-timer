<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { AlertBanner, ConfirmDialog, HelpTip } from "@rusty-timer/shared-ui";
  import { resizeWidth } from "$lib/actions/resizeWidth";
  import {
    store,
    streamIdentity,
    streamHasRecentActivity,
    streamIsOptimisticallySubscribing,
    toggleSubscription,
    subscribeAllAvailable,
    setStreamAnnouncerPublish,
    updateStreamEventType,
    changeEarliestEpoch,
    selectedEarliestEpochValue,
    formatEarliestEpochOption,
    refreshStreamsAndEpochOptions,
    clearStreamLocalMetrics,
    clearAllStreamsLocalMetrics,
    openHelp,
    getStreamRole,
    setStreamRole,
    getStreamDriftMs,
    getOrderedStreams,
    moveStreamOrder,
    cutoverStartFinishReaders,
    bulkSyncClocks,
    bulkAdvanceEpoch,
    bulkSetRole,
    loadConnections,
    type StreamRole,
  } from "$lib/store.svelte";
  import * as api from "$lib/api";
  import {
    type StreamEntry,
    resetStreamData,
    resetAllStreamsData,
    clearStreamBoth,
  } from "$lib/api";
  import ExportTagdataModal from "$lib/components/ExportTagdataModal.svelte";
  import ForwarderConfigModal from "./ForwarderConfigModal.svelte";
  import ForwarderWifiModal from "./ForwarderWifiModal.svelte";
  import { btnPrimary, btnSecondary } from "$lib/ui-classes";
  import {
    deriveStreamDisplayStatus,
    streamDisplayBadge,
    streamDisplayDotClass,
    streamDisplayLabel,
  } from "$lib/stream-status";

  let tableWidth = $state(0);
  let expandedKeys = $state<Set<string>>(new Set());
  let selectedKeys = $state<Set<string>>(new Set());
  let exportModalOpen = $state(false);
  let exportInitialStreamId = $state<string | null>(null);
  let selectedConfigEndpointId = $state<string | null>(null);
  let selectedWifiEndpointId = $state<string | null>(null);

  let selectedConfigForwarder = $derived(
    store.connections?.forwarders.find(
      (f) => f.endpoint_id === selectedConfigEndpointId,
    ) ?? null,
  );

  let selectedWifiForwarder = $derived(
    store.connections?.forwarders.find(
      (f) => f.endpoint_id === selectedWifiEndpointId,
    ) ?? null,
  );

  // Fixed column widths (approx)
  const READS_COL = 95;
  const DRIFT_COL = 85;
  const CHEVRON_COL = 32;
  const STREAM_COL_MIN = 180;
  const PADDING = 48;

  // Last Read field thresholds
  const TIME_W = 70;
  const BIB_W = 40;
  const NAME_W = 80;

  function availableLastReadWidth(): number {
    return Math.max(
      0,
      tableWidth -
        STREAM_COL_MIN -
        READS_COL -
        DRIFT_COL -
        CHEVRON_COL -
        PADDING,
    );
  }

  function showLastReadCol(): boolean {
    return availableLastReadWidth() >= TIME_W;
  }

  onMount(() => {
    if (!store.connections) {
      void loadConnections();
    }
  });

  let orderedStreams = $derived(
    store.streams ? getOrderedStreams(store.streams.streams) : [],
  );

  let cutoverCandidates = $derived(
    store.streams
      ? store.streams.streams.filter(
          (s) =>
            s.subscribed &&
            getStreamRole(s) === "start_finish" &&
            s.event_type === "start",
        )
      : [],
  );

  let selectedStreams = $derived(
    orderedStreams.filter((s) => selectedKeys.has(streamIdentity(s))),
  );

  let allSelected = $derived(
    orderedStreams.length > 0 && selectedKeys.size === orderedStreams.length,
  );

  function toggleExpand(stream: StreamEntry) {
    const key = streamIdentity(stream);
    const next = new Set(expandedKeys);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    expandedKeys = next;
  }

  function expandAll(expanded: boolean) {
    if (expanded && store.streams) {
      expandedKeys = new Set(store.streams.streams.map(streamIdentity));
    } else {
      expandedKeys = new Set();
    }
  }

  function toggleSelect(stream: StreamEntry, checked: boolean) {
    const key = streamIdentity(stream);
    const next = new Set(selectedKeys);
    if (checked) {
      next.add(key);
    } else {
      next.delete(key);
    }
    selectedKeys = next;
  }

  function toggleSelectAll(checked: boolean) {
    if (checked) {
      selectedKeys = new Set(orderedStreams.map(streamIdentity));
    } else {
      selectedKeys = new Set();
    }
  }

  function handleMoveStream(stream: StreamEntry, direction: -1 | 1) {
    moveStreamOrder(streamIdentity(stream), direction, orderedStreams);
  }

  function formatLastReadTimestamp(timestamp: string): string {
    const d = new Date(timestamp);
    if (isNaN(d.getTime())) return timestamp;
    const hh = String(d.getHours()).padStart(2, "0");
    const mm = String(d.getMinutes()).padStart(2, "0");
    const ss = String(d.getSeconds()).padStart(2, "0");
    const ms = String(d.getMilliseconds()).padStart(3, "0");
    return `${hh}:${mm}:${ss}.${ms}`;
  }

  function formatLag(lagMs: number | null): string {
    if (lagMs === null) return "N/A (no events yet)";
    if (lagMs < 1000) return `${lagMs} ms`;
    return `${(lagMs / 1000).toFixed(1)} s`;
  }

  function formatDuration(ms: number): string {
    if (ms < 1000) return "< 1s";
    const totalSeconds = Math.floor(ms / 1000);
    const hours = Math.floor(totalSeconds / 3600);
    const minutes = Math.floor((totalSeconds % 3600) / 60);
    const seconds = totalSeconds % 60;
    if (hours > 0) return `${hours}h ${minutes}m ${seconds}s`;
    if (minutes > 0) return `${minutes}m ${seconds}s`;
    return `${seconds}s`;
  }

  let timeSinceLastRead = $state<Record<string, string>>({});

  $effect(() => {
    if (expandedKeys.size === 0) return;

    const update = () => {
      const now = Date.now();
      const next: Record<string, string> = {
        ...untrack(() => timeSinceLastRead),
      };
      for (const key of expandedKeys) {
        const metrics = store.streamMetrics.get(key);
        if (!metrics?.epoch_last_received_at) {
          next[key] = "N/A (no events in epoch)";
        } else {
          const lastAt = new Date(metrics.epoch_last_received_at).getTime();
          next[key] = formatDuration(now - lastAt);
        }
      }
      timeSinceLastRead = next;
    };

    update();
    const interval = setInterval(update, 1000);
    return () => clearInterval(interval);
  });

  function streamPrimaryLabel(stream: StreamEntry): string {
    if (!stream.subscribed) return stream.stream_id;
    return stream.reader_ip ?? stream.stream_id;
  }

  function streamForwarderLabel(
    stream: StreamEntry,
  ): string | null | undefined {
    return stream.display_alias?.trim() || stream.forwarder_id;
  }

  function streamSecondaryLabel(stream: StreamEntry): string | null {
    if (stream.subscribed) return null;
    const alias = stream.display_alias?.trim();
    return alias ? alias : null;
  }

  let confirmReplayAllOpen = $state(false);
  let replayAllBusy = $state(false);
  let confirmingReplayStream = $state<StreamEntry | null>(null);
  let replayingStreamKey = $state<string | null>(null);
  let confirmingClearBothStream = $state<StreamEntry | null>(null);
  let clearingBothStreamKey = $state<string | null>(null);
  let actionError = $state<string | null>(null);

  // Single reader inline action states
  let epochInputs = $state<Record<string, string>>({});
  let advancingEpochKey = $state<string | null>(null);
  let syncingClockKey = $state<string | null>(null);

  // Cutover action state
  let confirmCutoverOpen = $state(false);
  let cutoverEpochInput = $derived(store.cutoverEpochName || "Finish Reads");
  let cutoverBusy = $state(false);

  // Bulk action states
  let confirmBulkEpochOpen = $state(false);
  let bulkEpochInput = $state("");
  let bulkEpochBusy = $state(false);
  let bulkSyncBusy = $state(false);
  let bulkRoleBusy = $state(false);

  async function handleReplayAll() {
    confirmReplayAllOpen = false;
    replayAllBusy = true;
    actionError = null;
    try {
      await resetAllStreamsData();
      clearAllStreamsLocalMetrics();
      await refreshStreamsAndEpochOptions();
    } catch (e) {
      actionError = `Failed to replay all streams: ${String(e)}`;
    } finally {
      replayAllBusy = false;
    }
  }

  async function handleReplayStream(stream: StreamEntry) {
    const key = streamIdentity(stream);
    confirmingReplayStream = null;
    replayingStreamKey = key;
    actionError = null;
    try {
      await resetStreamData({
        forwarder_endpoint_id: stream.forwarder_endpoint_id,
        stream_id: stream.stream_id,
      });
      clearStreamLocalMetrics(key);
      await refreshStreamsAndEpochOptions();
    } catch (e) {
      actionError = `Failed to replay stream ${streamPrimaryLabel(stream)}: ${String(e)}`;
    } finally {
      replayingStreamKey = null;
    }
  }

  async function handleClearBoth(stream: StreamEntry) {
    const key = streamIdentity(stream);
    confirmingClearBothStream = null;
    clearingBothStreamKey = key;
    actionError = null;
    try {
      await clearStreamBoth(stream.forwarder_endpoint_id, stream.stream_id);
      clearStreamLocalMetrics(key);
      await refreshStreamsAndEpochOptions();
    } catch (e) {
      actionError = `Failed to clear stream ${streamPrimaryLabel(stream)}: ${String(e)}`;
    } finally {
      clearingBothStreamKey = null;
    }
  }

  async function handleSyncSingleClock(stream: StreamEntry) {
    const key = streamIdentity(stream);
    syncingClockKey = key;
    actionError = null;
    try {
      await api.readerSyncClock(stream.forwarder_endpoint_id, stream.stream_id);
      await loadConnections();
    } catch (e) {
      actionError = `Failed to sync clock for ${streamPrimaryLabel(stream)}: ${String(e)}`;
    } finally {
      syncingClockKey = null;
    }
  }

  async function handleAdvanceSingleEpoch(stream: StreamEntry) {
    const key = streamIdentity(stream);
    advancingEpochKey = key;
    actionError = null;
    const epochName = epochInputs[key]?.trim() || null;
    try {
      await api.readerAdvanceEpoch(
        stream.forwarder_endpoint_id,
        stream.stream_id,
        epochName,
      );
      clearStreamLocalMetrics(key);
      await refreshStreamsAndEpochOptions();
      await loadConnections();
    } catch (e) {
      actionError = `Failed to advance epoch for ${streamPrimaryLabel(stream)}: ${String(e)}`;
    } finally {
      advancingEpochKey = null;
    }
  }

  async function handleRoleChange(stream: StreamEntry, role: StreamRole) {
    actionError = null;
    setStreamRole(stream, role);
    if (!stream.subscribed) return;

    try {
      if (
        role === "start" ||
        (role === "start_finish" && stream.event_type !== "finish")
      ) {
        await updateStreamEventType(stream, "start");
      } else if (role === "finish") {
        await updateStreamEventType(stream, "finish");
      } else if (role === "announcer") {
        await updateStreamEventType(stream, "exclude");
        await setStreamAnnouncerPublish(stream, true);
      }
    } catch (e) {
      actionError = `Failed to update role for ${streamPrimaryLabel(stream)}: ${String(e)}`;
    }
  }

  async function handleCutoverConfirm() {
    confirmCutoverOpen = false;
    cutoverBusy = true;
    actionError = null;
    try {
      await cutoverStartFinishReaders(cutoverEpochInput);
    } catch (e) {
      actionError = `Cutover failed: ${String(e)}`;
    } finally {
      cutoverBusy = false;
    }
  }

  async function handleBulkSyncClocks() {
    if (selectedStreams.length === 0) return;
    bulkSyncBusy = true;
    actionError = null;
    try {
      await bulkSyncClocks(selectedStreams);
    } catch (e) {
      actionError = `Bulk sync clocks failed: ${String(e)}`;
    } finally {
      bulkSyncBusy = false;
    }
  }

  async function handleBulkAdvanceEpoch() {
    confirmBulkEpochOpen = false;
    bulkEpochBusy = true;
    actionError = null;
    try {
      await bulkAdvanceEpoch(selectedStreams, bulkEpochInput);
    } catch (e) {
      actionError = `Bulk advance epoch failed: ${String(e)}`;
    } finally {
      bulkEpochBusy = false;
    }
  }

  async function handleBulkSetRole(role: StreamRole) {
    if (selectedStreams.length === 0) return;
    bulkRoleBusy = true;
    actionError = null;
    try {
      await bulkSetRole(selectedStreams, role);
    } catch (e) {
      actionError = `Bulk set role failed: ${String(e)}`;
    } finally {
      bulkRoleBusy = false;
    }
  }

  function batteryDisplay(stream: StreamEntry): string {
    if (stream.forwarder_id && store.upsState.get(stream.forwarder_id)) {
      const ups = store.upsState.get(stream.forwarder_id);
      const pct = ups?.status?.battery_percent;
      const charging = ups?.status?.charging;
      if (pct != null) {
        return `${pct}% ${charging ? "(charging ⚡)" : "(on battery)"}`;
      }
    }
    if (store.connections) {
      const forwarder = store.connections.forwarders.find(
        (f) => f.endpoint_id === stream.forwarder_endpoint_id,
      );
      if (forwarder?.ups?.battery_percent != null) {
        return `${forwarder.ups.battery_percent}% ${!forwarder.ups.on_battery ? "(charging ⚡)" : "(on battery)"}`;
      }
    }
    return "\u2014";
  }

  function formatOptional(value: string | null | undefined): string {
    return value ?? "\u2014";
  }

  function formatLastRead(key: string): string {
    const read = store.lastReads.get(key);
    if (!read) return "\u2014";

    const avail = availableLastReadWidth();
    let used = 0;
    const parts: string[] = [];

    // Time (highest priority)
    if (used + TIME_W <= avail) {
      parts.push(formatLastReadTimestamp(read.timestamp));
      used += TIME_W;
    } else {
      return "\u2014";
    }

    if (read.name) {
      // Has participant name → show bib + name, no chip ID
      if (read.bib && used + BIB_W <= avail) {
        parts.push(`#${read.bib}`);
        used += BIB_W;
      }
      if (used + NAME_W <= avail) {
        parts.push(read.name);
        used += NAME_W;
      }
    } else if (read.bib) {
      // Has bib but no name → show an explicit unknown-participant label.
      if (used + NAME_W <= avail) {
        parts.push(`Unknown Participant ${read.bib}`);
        used += NAME_W;
      }
    } else {
      // No bib and no name → show "Unknown Chip <chip-id>"
      const cleaned = read.chip_id.replaceAll(":", "");
      const label = `Unknown Chip ${cleaned}`;
      if (used + NAME_W <= avail) {
        parts.push(label);
        used += NAME_W;
      }
    }

    return parts.join("  ");
  }
</script>

<div class="h-full flex flex-col" use:resizeWidth={(w) => (tableWidth = w)}>
  {#if store.streams?.upstream_error}
    <div class="px-4 py-2">
      <AlertBanner variant="warn" message={store.streams.upstream_error} />
    </div>
  {/if}

  {#if actionError}
    <div class="px-4 py-2">
      <AlertBanner
        variant="err"
        message={actionError}
        onDismiss={() => {
          actionError = null;
        }}
      />
    </div>
  {/if}

  {#if store.streams?.streams && store.streams.streams.length > 0}
    <!-- Action Toolbar (Multi-select, Bulk Actions, Cutover Button, Collapse/Expand) -->
    <div
      class="flex flex-wrap items-center justify-between gap-3 px-4 py-2 border-b border-border bg-surface-0 shrink-0 text-sm"
    >
      <!-- Left: Multi-select & Bulk Controls -->
      <div class="flex items-center gap-3">
        <label
          class="flex items-center gap-2 cursor-pointer font-medium text-xs text-text-secondary select-none"
        >
          <input
            type="checkbox"
            checked={allSelected}
            onchange={(e) => toggleSelectAll(e.currentTarget.checked)}
            class="rounded accent-accent cursor-pointer"
            aria-label="Select all streams"
          />
          <span>Select All ({selectedKeys.size}/{orderedStreams.length})</span>
        </label>

        <div class="h-4 w-px bg-border"></div>

        <!-- Bulk Actions Group -->
        <div
          class="flex items-center gap-1.5 transition-opacity text-xs {selectedKeys.size >
          0
            ? ''
            : 'opacity-50 pointer-events-none'}"
        >
          <button
            type="button"
            class={btnSecondary}
            class:!px-2.5={true}
            class:!py-1={true}
            class:!text-xs={true}
            disabled={selectedKeys.size === 0 || bulkSyncBusy}
            onclick={handleBulkSyncClocks}
          >
            {bulkSyncBusy ? "Syncing…" : "Sync Clocks"}
          </button>
          <button
            type="button"
            class={btnSecondary}
            class:!px-2.5={true}
            class:!py-1={true}
            class:!text-xs={true}
            disabled={selectedKeys.size === 0}
            onclick={() => {
              bulkEpochInput = "";
              confirmBulkEpochOpen = true;
            }}
          >
            Advance Epoch…
          </button>
          <button
            type="button"
            class={btnSecondary}
            class:!px-2.5={true}
            class:!py-1={true}
            class:!text-xs={true}
            disabled={selectedKeys.size === 0}
            onclick={() => {
              exportInitialStreamId = null;
              exportModalOpen = true;
            }}
          >
            Export TAGDATA
          </button>
          <select
            class="px-2 py-1 text-xs rounded font-medium bg-surface-2 border border-border text-text-primary cursor-pointer disabled:opacity-50"
            disabled={selectedKeys.size === 0 || bulkRoleBusy}
            onchange={(e) => {
              const val = e.currentTarget.value as StreamRole;
              if (val) void handleBulkSetRole(val);
              e.currentTarget.value = "";
            }}
          >
            <option value="" disabled selected>Set Role…</option>
            <option value="start">Set: Start</option>
            <option value="finish">Set: Finish</option>
            <option value="start_finish">Set: Start / Finish</option>
            <option value="announcer">Set: Announcer</option>
          </select>
        </div>
      </div>

      <!-- Right: Cutover Button, Export, Collapse/Expand -->
      <div class="flex items-center gap-2">
        {#if cutoverCandidates.length > 0}
          <button
            data-testid="advance-starts-to-finish-btn"
            type="button"
            class="flex items-center gap-1.5 px-3 py-1 text-xs font-medium rounded-md bg-amber-600 hover:bg-amber-700 text-white cursor-pointer shadow-sm disabled:opacity-50"
            disabled={store.streamActionBusy || cutoverBusy}
            onclick={() => {
              confirmCutoverOpen = true;
            }}
          >
            <svg
              class="w-3.5 h-3.5 text-amber-200"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2.5"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <polygon points="13 19 22 12 13 5 13 19"></polygon>
              <polygon points="2 19 11 12 2 5 2 19"></polygon>
            </svg>
            <span>Advance Starts to Finish</span>
            <span
              class="bg-amber-800/60 text-white px-1 py-0.2 rounded text-[10px] font-mono"
            >
              {cutoverCandidates.length}
            </span>
          </button>
        {/if}

        <button
          data-testid="export-tagdata-btn"
          class={btnSecondary}
          class:!px-2.5={true}
          class:!py-1={true}
          class:!text-xs={true}
          onclick={() => {
            exportInitialStreamId = null;
            exportModalOpen = true;
          }}
        >
          Export TAGDATA
        </button>

        {#if store.streams.streams.some((stream) => !stream.subscribed)}
          <button
            data-testid="subscribe-all-btn"
            class={btnPrimary}
            class:!px-2.5={true}
            class:!py-1={true}
            class:!text-xs={true}
            onclick={() => void subscribeAllAvailable()}
            disabled={store.streamActionBusy}
          >
            Subscribe All
          </button>
          <HelpTip
            fieldKey="subscribe_all"
            sectionKey="streams"
            context="receiver"
            onOpenModal={openHelp}
          />
        {/if}

        {#if store.streams.streams.some((stream) => stream.subscribed)}
          <button
            data-testid="replay-all-btn"
            class={btnSecondary}
            class:!px-2.5={true}
            class:!py-1={true}
            class:!text-xs={true}
            onclick={() => {
              confirmReplayAllOpen = true;
            }}
            disabled={store.streamActionBusy || replayAllBusy}
          >
            {replayAllBusy ? "Replaying..." : "Replay All Reads"}
          </button>
          <HelpTip
            fieldKey="replay_all_reads"
            sectionKey="streams"
            context="receiver"
            onOpenModal={openHelp}
          />
        {/if}

        <div class="h-4 w-px bg-border mx-1"></div>

        <button
          type="button"
          onclick={() => expandAll(true)}
          class="text-xs text-text-muted hover:text-text-primary cursor-pointer"
        >
          Expand All
        </button>
        <span class="text-text-muted text-xs">&middot;</span>
        <button
          type="button"
          onclick={() => expandAll(false)}
          class="text-xs text-text-muted hover:text-text-primary cursor-pointer"
        >
          Collapse All
        </button>
      </div>
    </div>
  {/if}

  {#if !store.streams || store.streams.streams.length === 0}
    <p class="px-4 py-8 text-sm text-text-muted text-center m-0">
      No streams available.
    </p>
  {:else}
    <div class="flex-1 overflow-y-auto">
      <table class="w-full text-sm border-collapse">
        <thead>
          <tr
            class="sticky top-0 z-10 bg-surface-0 border-b border-border text-left text-text-muted text-xs select-none"
          >
            <th class="w-px whitespace-nowrap py-2 px-4 font-medium">
              <span class="inline-flex items-center">
                Stream
                <HelpTip
                  fieldKey="stream_identity"
                  sectionKey="streams"
                  context="receiver"
                  onOpenModal={openHelp}
                />
              </span>
            </th>
            {#if showLastReadCol()}
              <th class="w-full py-2 px-2 font-medium text-left">
                <span class="inline-flex items-center">
                  Last Read
                  <HelpTip
                    fieldKey="last_read"
                    sectionKey="streams"
                    context="receiver"
                    onOpenModal={openHelp}
                  />
                </span>
              </th>
            {/if}
            <th class="py-2 px-2 font-medium text-center w-[85px]"> Drift </th>
            <th class="py-2 px-4 font-medium text-right w-[95px]">
              <span class="inline-flex items-center justify-end">
                Reads
                <HelpTip
                  fieldKey="reads"
                  sectionKey="streams"
                  context="receiver"
                  onOpenModal={openHelp}
                />
              </span>
            </th>
            <th class="w-[32px] py-2 pr-4"></th>
          </tr>
        </thead>
        <tbody>
          {#each orderedStreams as stream, index (streamIdentity(stream))}
            {@const key = streamIdentity(stream)}
            {@const primaryLabel = streamPrimaryLabel(stream)}
            {@const secondaryLabel = streamSecondaryLabel(stream)}
            {@const role = getStreamRole(stream)}
            {@const driftMs = getStreamDriftMs(stream)}
            {@const displayStatus = deriveStreamDisplayStatus(stream, {
              recentActivity: streamHasRecentActivity(stream),
              optimisticSubscribing: streamIsOptimisticallySubscribing(stream),
            })}
            {@const displayBadge = streamDisplayBadge(displayStatus)}
            <!-- Collapsed Double-Height Row -->
            <tr
              class="border-b border-border/50 hover:bg-surface-1/50 cursor-pointer {stream.subscribed
                ? ''
                : 'bg-surface-1/20'}"
              role="button"
              aria-expanded={expandedKeys.has(key)}
              tabindex="0"
              onclick={() => toggleExpand(stream)}
              onkeydown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  toggleExpand(stream);
                }
              }}
            >
              <!-- Col 1: Stream Identity, Reorder, Checkbox, Role -->
              <td class="w-px whitespace-nowrap py-2.5 px-4 align-middle">
                <div
                  class="flex items-center gap-2.5 min-w-0"
                  role="presentation"
                  onclick={(e) => e.stopPropagation()}
                  onkeydown={(e) => e.stopPropagation()}
                >
                  <!-- Move Up/Down Controls -->
                  <div class="flex flex-col gap-0.5 text-text-muted">
                    <button
                      type="button"
                      class="p-0.5 leading-none hover:text-text-primary disabled:opacity-20 cursor-pointer disabled:cursor-not-allowed"
                      disabled={index === 0}
                      title="Move earlier in course order"
                      aria-label="Move earlier in course order"
                      onclick={() => handleMoveStream(stream, -1)}
                    >
                      <svg
                        class="w-3 h-3"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2.5"
                      >
                        <polyline points="18 15 12 9 6 15"></polyline>
                      </svg>
                    </button>
                    <button
                      type="button"
                      class="p-0.5 leading-none hover:text-text-primary disabled:opacity-20 cursor-pointer disabled:cursor-not-allowed"
                      disabled={index === orderedStreams.length - 1}
                      title="Move later in course order"
                      aria-label="Move later in course order"
                      onclick={() => handleMoveStream(stream, 1)}
                    >
                      <svg
                        class="w-3 h-3"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2.5"
                      >
                        <polyline points="6 9 12 15 18 9"></polyline>
                      </svg>
                    </button>
                  </div>

                  <!-- Selection Checkbox -->
                  <input
                    type="checkbox"
                    checked={selectedKeys.has(key)}
                    onchange={(e) =>
                      toggleSelect(stream, e.currentTarget.checked)}
                    class="rounded accent-accent cursor-pointer"
                    aria-label={`Select stream ${primaryLabel}`}
                  />

                  <!-- Connection Status Dot -->
                  <span
                    class="w-2.5 h-2.5 rounded-full shrink-0 {streamDisplayDotClass(
                      displayStatus,
                    )}"
                    title={streamDisplayLabel(displayStatus)}
                    aria-label={streamDisplayLabel(displayStatus)}
                  ></span>

                  <!-- Double-height Info: Line 1 = IP & Role Badge, Line 2 = Forwarder & Epoch Name -->
                  <div class="min-w-0">
                    <div class="flex items-center gap-2">
                      <span
                        class={stream.subscribed
                          ? "text-text-primary font-medium"
                          : "text-text-secondary font-mono"}
                      >
                        {primaryLabel}
                      </span>

                      {#if stream.failure}
                        <span
                          data-testid="stream-halted-{key}"
                          class="rounded border border-status-err-border bg-status-err-bg px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-status-err"
                          title={`Stream halted: ${stream.failure.reason}${stream.failure.seq != null ? ` (seq ${stream.failure.seq})` : ""} — see the operator runbook. Delivery stays stopped until the receiver reconnects or the subscription changes.`}
                        >
                          halted
                        </span>
                      {/if}

                      {#if !stream.subscribed}
                        <span
                          class="rounded border border-border px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-text-muted"
                        >
                          Available
                        </span>
                      {:else}
                        {#if role === "start_finish"}
                          {#if stream.event_type === "start"}
                            <span
                              class="rounded border border-amber-500/30 bg-amber-500/10 px-1.5 py-0.5 text-[10px] uppercase font-semibold tracking-wide text-amber-700 dark:text-amber-400"
                            >
                              Start / Finish &middot; START
                            </span>
                          {:else}
                            <span
                              class="rounded border border-emerald-500/30 bg-emerald-500/10 px-1.5 py-0.5 text-[10px] uppercase font-semibold tracking-wide text-emerald-700 dark:text-emerald-400"
                            >
                              Start / Finish &middot; FINISH
                            </span>
                          {/if}
                        {:else if role === "start"}
                          <span
                            class="rounded border border-blue-500/30 bg-blue-500/10 px-1.5 py-0.5 text-[10px] uppercase font-semibold tracking-wide text-blue-700 dark:text-blue-400"
                          >
                            Start
                          </span>
                        {:else if role === "finish"}
                          <span
                            class="rounded border border-emerald-500/30 bg-emerald-500/10 px-1.5 py-0.5 text-[10px] uppercase font-semibold tracking-wide text-emerald-700 dark:text-emerald-400"
                          >
                            Finish
                          </span>
                        {:else if role === "announcer"}
                          <span
                            class="rounded border border-purple-500/30 bg-purple-500/10 px-1.5 py-0.5 text-[10px] uppercase font-semibold tracking-wide text-purple-700 dark:text-purple-400"
                          >
                            Announcer
                          </span>
                        {/if}

                        {#if displayBadge}
                          <span
                            class="rounded border border-status-warn-border bg-status-warn-bg px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-status-warn"
                          >
                            {displayBadge}
                          </span>
                        {/if}
                      {/if}
                    </div>

                    <!-- Line 2: Forwarder ID & Epoch Name or secondary alias -->
                    <div
                      class="flex items-center gap-2 text-xs text-text-muted mt-0.5 truncate"
                    >
                      {#if stream.subscribed}
                        {#if streamForwarderLabel(stream)}
                          <span class="font-mono"
                            >{streamForwarderLabel(stream)}</span
                          >
                        {/if}
                        {#if stream.stream_epoch != null}
                          {#if streamForwarderLabel(stream)}<span>&middot;</span
                            >{/if}
                          <span>
                            Epoch #{stream.stream_epoch}{stream.current_epoch_name
                              ? `: ${stream.current_epoch_name.trim()}`
                              : ""}
                          </span>
                        {/if}
                      {:else if secondaryLabel}
                        <span class="truncate">{secondaryLabel}</span>
                      {/if}
                    </div>
                  </div>
                </div>
              </td>

              <!-- Col 2: Last Read -->
              {#if showLastReadCol()}
                <td
                  class="w-full max-w-0 py-2.5 px-2 text-left text-text-muted font-mono truncate align-middle"
                >
                  {formatLastRead(key)}
                </td>
              {/if}

              <!-- Col 3: Drift Chip -->
              <td
                class="w-[85px] py-2.5 px-2 text-center align-middle whitespace-nowrap"
              >
                {#if driftMs != null}
                  {@const abs = Math.abs(driftMs)}
                  <span
                    class="font-mono text-xs px-1.5 py-0.5 rounded border font-medium inline-block text-center min-w-[56px] {abs <
                    250
                      ? 'border-status-ok-border bg-status-ok-bg text-status-ok'
                      : abs < 1000
                        ? 'border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-400 font-bold'
                        : 'border-status-err-border bg-status-err-bg text-status-err font-bold'}"
                    title="Reader clock offset from receiver host"
                  >
                    {driftMs >= 0 ? "+" : ""}{driftMs}ms
                  </span>
                {:else}
                  <span class="text-xs text-text-muted">&mdash;</span>
                {/if}
              </td>

              <!-- Col 4: Reads (Epoch) -->
              <td
                class="py-2.5 px-4 text-right font-mono text-text-primary font-bold text-sm w-[95px] align-middle whitespace-nowrap"
              >
                {stream.subscribed
                  ? (stream.reads_epoch ?? 0).toLocaleString()
                  : "\u2014"}
              </td>

              <!-- Col 5: Expand Chevron -->
              <td
                class="w-[32px] py-2.5 pr-4 text-right align-middle text-text-muted"
              >
                <svg
                  class="w-4 h-4 inline-block transition-transform duration-150 {expandedKeys.has(
                    key,
                  )
                    ? 'rotate-180 text-accent'
                    : ''}"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  aria-hidden="true"
                >
                  <polyline points="6 9 12 15 18 9"></polyline>
                </svg>
              </td>
            </tr>

            <!-- Expanded Details Row -->
            {#if expandedKeys.has(key)}
              {@const metrics = store.streamMetrics.get(key)}
              <tr>
                <td colspan={showLastReadCol() ? 5 : 4} class="p-0">
                  <div
                    class="bg-surface-1 px-4 py-3 border-b border-border space-y-3"
                  >
                    <!-- Quick Action Controls for this Reader -->
                    <div
                      class="flex flex-wrap items-center justify-between gap-3 bg-surface-0 p-2.5 rounded-lg border border-border text-xs"
                    >
                      <div class="flex flex-wrap items-center gap-2">
                        <!-- Sync Clock button -->
                        <button
                          data-testid={`sync-clock-${key}`}
                          class={btnSecondary}
                          class:!px-2.5={true}
                          class:!py-1={true}
                          class:!text-xs={true}
                          onclick={(e) => {
                            e.stopPropagation();
                            void handleSyncSingleClock(stream);
                          }}
                          disabled={syncingClockKey === key ||
                            store.streamActionBusy}
                        >
                          {syncingClockKey === key ? "Syncing…" : "Sync Clock"}
                        </button>

                        <!-- Inline Advance Epoch -->
                        <div
                          class="inline-flex items-center gap-1 bg-surface-1 p-0.5 rounded border border-border"
                        >
                          <input
                            type="text"
                            data-testid={`epoch-name-input-${key}`}
                            placeholder="New epoch name"
                            value={epochInputs[key] ?? ""}
                            oninput={(e) => {
                              epochInputs = {
                                ...epochInputs,
                                [key]: e.currentTarget.value,
                              };
                            }}
                            onclick={(e) => e.stopPropagation()}
                            class="px-2 py-0.5 text-xs rounded border border-border bg-surface-0 text-text-primary font-mono w-36 focus:outline-none focus:ring-1 focus:ring-accent"
                          />
                          <button
                            data-testid={`advance-epoch-btn-${key}`}
                            class="px-2 py-0.5 text-xs font-medium rounded bg-amber-600 hover:bg-amber-700 text-white cursor-pointer disabled:opacity-50"
                            disabled={advancingEpochKey === key ||
                              store.streamActionBusy}
                            onclick={(e) => {
                              e.stopPropagation();
                              void handleAdvanceSingleEpoch(stream);
                            }}
                          >
                            {advancingEpochKey === key
                              ? "Advancing…"
                              : "Advance Epoch"}
                          </button>
                        </div>

                        <!-- Export TAGDATA -->
                        <button
                          data-testid={`export-stream-tagdata-${key}`}
                          class={btnSecondary}
                          class:!px-2.5={true}
                          class:!py-1={true}
                          class:!text-xs={true}
                          onclick={(e) => {
                            e.stopPropagation();
                            exportInitialStreamId = key;
                            exportModalOpen = true;
                          }}
                        >
                          Export TAGDATA
                        </button>

                        <!-- Clear Reads -->
                        {#if stream.subscribed}
                          <button
                            data-testid={`clear-both-stream-${key}`}
                            class="px-2.5 py-1 text-xs font-medium rounded-md bg-transparent text-status-err border border-status-err hover:bg-status-err-bg cursor-pointer disabled:opacity-50"
                            onclick={(e) => {
                              e.stopPropagation();
                              confirmingClearBothStream = stream;
                            }}
                            disabled={replayingStreamKey === key ||
                              clearingBothStreamKey === key}
                            title="Purge stored reads from forwarder and reset receiver local data"
                          >
                            {clearingBothStreamKey === key
                              ? "Clearing…"
                              : "Clear Reads"}
                          </button>
                          <HelpTip
                            fieldKey="clear_both"
                            sectionKey="streams"
                            context="receiver"
                            onOpenModal={openHelp}
                          />
                        {/if}

                        <!-- Subscribe / Unsubscribe -->
                        <button
                          data-testid={`subscribe-toggle-${key}`}
                          class={stream.subscribed ? btnSecondary : btnPrimary}
                          class:!px-2.5={true}
                          class:!py-1={true}
                          class:!text-xs={true}
                          onclick={(e) => {
                            e.stopPropagation();
                            void toggleSubscription(stream);
                          }}
                          disabled={store.streamActionBusy}
                        >
                          {stream.subscribed ? "Unsubscribe" : "Subscribe"}
                        </button>
                        <HelpTip
                          fieldKey="subscribed"
                          sectionKey="streams"
                          context="receiver"
                          onOpenModal={openHelp}
                        />

                        <!-- Replay Reads -->
                        {#if stream.subscribed}
                          <button
                            data-testid={`replay-stream-${key}`}
                            class={btnSecondary}
                            class:!px-2.5={true}
                            class:!py-1={true}
                            class:!text-xs={true}
                            onclick={(e) => {
                              e.stopPropagation();
                              confirmingReplayStream = stream;
                            }}
                            disabled={replayingStreamKey === key ||
                              clearingBothStreamKey === key}
                            title="Clear local stream data and replay all retained reads from the forwarder"
                          >
                            {replayingStreamKey === key
                              ? "Replaying…"
                              : "Replay Reads"}
                          </button>
                          <HelpTip
                            fieldKey="replay_reads"
                            sectionKey="streams"
                            context="receiver"
                            onOpenModal={openHelp}
                          />
                        {/if}
                      </div>

                      <!-- Role Select & Announce Toggle -->
                      <div class="flex items-center gap-3">
                        {#if stream.subscribed}
                          <label
                            class="flex items-center gap-1.5 font-medium text-text-secondary"
                          >
                            <span>Role:</span>
                            <select
                              data-testid={`dbf-event-type-${key}`}
                              class="px-2 py-1 text-xs rounded font-mono bg-surface-1 border border-border text-text-primary focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-50"
                              value={role === "announcer" ? "exclude" : role}
                              onchange={(e) => {
                                e.stopPropagation();
                                const val = e.currentTarget.value;
                                void handleRoleChange(
                                  stream,
                                  val === "exclude"
                                    ? "announcer"
                                    : (val as StreamRole),
                                );
                              }}
                              onclick={(e) => e.stopPropagation()}
                              disabled={store.streamEventTypeBusy[key] ||
                                store.streamActionBusy}
                            >
                              <option value="start_finish"
                                >Start / Finish</option
                              >
                              <option value="start">Start</option>
                              <option value="finish">Finish</option>
                              <option value="exclude">Exclude</option>
                            </select>
                            <HelpTip
                              fieldKey="event_type"
                              sectionKey="streams"
                              context="receiver"
                              onOpenModal={openHelp}
                            />
                          </label>

                          <label
                            class="inline-flex items-center gap-1.5 text-xs text-text-muted cursor-pointer"
                            title="Publish this stream's reads to the announcer board"
                          >
                            <input
                              data-testid={`announcer-publish-toggle-${key}`}
                              type="checkbox"
                              checked={stream.announcer_publish ?? false}
                              disabled={store.streamAnnouncerBusy[key]}
                              onclick={(e) => e.stopPropagation()}
                              onchange={(e) => {
                                e.stopPropagation();
                                void setStreamAnnouncerPublish(
                                  stream,
                                  e.currentTarget.checked,
                                );
                              }}
                              class="accent-accent"
                            />
                            Announce
                            <HelpTip
                              fieldKey="announce"
                              sectionKey="streams"
                              context="receiver"
                              onOpenModal={openHelp}
                            />
                          </label>
                        {/if}
                      </div>
                    </div>

                    <!-- Clean Details Grid -->
                    <div class="grid grid-cols-2 gap-x-6 gap-y-2 text-xs">
                      <div>
                        <span class="text-text-muted">Reader IP:</span>
                        <span class="font-mono text-text-primary ml-1">
                          {formatOptional(stream.reader_ip)}
                        </span>
                      </div>
                      <div>
                        <span class="text-text-muted">
                          Forwarder:
                          <HelpTip
                            fieldKey="stream_identity"
                            sectionKey="streams"
                            context="receiver"
                            onOpenModal={openHelp}
                          />
                        </span>
                        <span class="font-mono text-text-primary ml-1">
                          {formatOptional(streamForwarderLabel(stream))}
                        </span>
                        <button
                          type="button"
                          data-testid={`stream-fwd-config-btn-${key}`}
                          class="ml-2 px-1.5 py-0.5 rounded text-[10px] font-medium border border-border bg-surface-1 hover:bg-surface-2 text-text-secondary cursor-pointer"
                          onclick={(e) => {
                            e.stopPropagation();
                            selectedConfigEndpointId =
                              stream.forwarder_endpoint_id;
                          }}
                        >
                          Configure Box
                        </button>
                        {#if store.connections?.forwarders.find((f) => f.endpoint_id === stream.forwarder_endpoint_id)?.wifi_control_available}
                          <button
                            type="button"
                            data-testid={`stream-fwd-wifi-btn-${key}`}
                            class="ml-1 px-1.5 py-0.5 rounded text-[10px] font-medium border border-border bg-surface-1 hover:bg-surface-2 text-text-secondary cursor-pointer"
                            onclick={(e) => {
                              e.stopPropagation();
                              selectedWifiEndpointId =
                                stream.forwarder_endpoint_id;
                            }}
                          >
                            Wi-Fi
                          </button>
                        {/if}
                      </div>
                      <div>
                        <span class="text-text-muted">Local Proxy:</span>
                        <span class="font-mono text-text-primary ml-1">
                          {stream.local_port
                            ? `127.0.0.1:${stream.local_port}`
                            : "not subscribed"}
                        </span>
                      </div>
                      <div>
                        <span class="text-text-muted">Forwarder Battery:</span>
                        <span class="font-mono text-text-primary ml-1">
                          {batteryDisplay(stream)}
                        </span>
                      </div>
                    </div>

                    {#if metrics}
                      <div class="mt-2 grid grid-cols-2 gap-x-8">
                        <!-- Lifetime Metrics -->
                        <div>
                          <p class="text-muted text-xs font-medium mb-1">
                            Lifetime
                            <HelpTip
                              fieldKey="stream_metrics"
                              sectionKey="streams"
                              context="receiver"
                              onOpenModal={openHelp}
                            />
                          </p>
                          <div class="grid grid-cols-1 gap-y-1.5 text-xs">
                            <div
                              title="Total frames received including retransmits"
                            >
                              <span class="text-text-muted">Raw count:</span>
                              <span class="font-mono text-text-primary ml-1">
                                {metrics.raw_count.toLocaleString()}
                              </span>
                            </div>
                            <div title="Unique frames after deduplication">
                              <span class="text-text-muted">Dedup count:</span>
                              <span class="font-mono text-text-primary ml-1">
                                {metrics.dedup_count.toLocaleString()}
                              </span>
                            </div>
                            <div
                              title="Duplicate frames that matched existing events"
                            >
                              <span class="text-text-muted">Retransmit:</span>
                              <span class="font-mono text-text-primary ml-1">
                                {metrics.retransmit_count.toLocaleString()}
                              </span>
                            </div>
                            <div
                              title="Forwarder-reported delay since the last unique frame was received (snapshot, not live)"
                            >
                              <span class="text-text-muted">Lag:</span>
                              <span class="font-mono text-text-primary ml-1">
                                {formatLag(metrics.lag_ms)}
                              </span>
                            </div>
                          </div>
                        </div>

                        <!-- Current Epoch Metrics (Cleaned: no raw frames, deduplicated, retransmitted) -->
                        <div>
                          <p class="text-muted text-xs font-medium mb-1">
                            Current Epoch
                            <HelpTip
                              fieldKey="stream_metrics"
                              sectionKey="streams"
                              context="receiver"
                              onOpenModal={openHelp}
                            />
                          </p>
                          <div class="grid grid-cols-1 gap-y-1.5 text-xs">
                            <div
                              title="Distinct chip IDs detected in the current epoch"
                            >
                              <span class="text-text-muted">Unique chips:</span>
                              <span class="font-mono text-text-primary ml-1">
                                {metrics.unique_chips.toLocaleString()}
                              </span>
                            </div>
                            <div
                              title="Live-updating elapsed time since last unique frame"
                            >
                              <span class="text-text-muted"
                                >Time since last read:</span
                              >
                              <span class="font-mono text-text-primary ml-1">
                                {timeSinceLastRead[key] ?? "—"}
                              </span>
                            </div>
                            <div
                              title="Timestamp of the last unique frame in the current epoch"
                            >
                              <span class="text-text-muted">Last read:</span>
                              <span class="font-mono text-text-primary ml-1">
                                {metrics.epoch_last_received_at
                                  ? new Date(
                                      metrics.epoch_last_received_at,
                                    ).toLocaleString()
                                  : "N/A (no events in epoch)"}
                              </span>
                            </div>
                          </div>
                        </div>
                      </div>
                    {:else}
                      <p class="text-muted text-xs mt-2">Metrics unavailable</p>
                    {/if}

                    <!-- From Epoch Override (if diagnostic override used) -->
                    {#if store.modeDraft !== "race"}
                      {@const options = store.streamEpochOptions[key] ?? []}
                      {@const selectedEarliest =
                        selectedEarliestEpochValue(stream)}
                      <div
                        class="pt-2 border-t border-border flex items-center gap-2"
                      >
                        <label
                          class="text-xs text-text-secondary flex items-center gap-2"
                        >
                          <span>From epoch:</span>
                          <HelpTip
                            fieldKey="earliest_epoch"
                            sectionKey="streams"
                            context="receiver"
                            onOpenModal={openHelp}
                          />
                          <select
                            data-testid={`earliest-epoch-${key}`}
                            class="px-2 py-1 text-xs rounded font-mono bg-surface-0 border border-border text-text-primary w-64 max-w-full focus:outline-none focus:ring-1 focus:ring-accent disabled:opacity-50"
                            value={selectedEarliest}
                            onchange={(e) => {
                              e.stopPropagation();
                              void changeEarliestEpoch(
                                stream,
                                e.currentTarget.value,
                              );
                            }}
                            onclick={(e) => e.stopPropagation()}
                            disabled={store.earliestEpochSaving[key]}
                          >
                            {#if store.earliestEpochLoading[key]}
                              <option value="">Loading epochs...</option>
                            {:else if store.earliestEpochLoadErrors[key]}
                              <option value="">Epochs unavailable</option>
                            {:else if options.length === 0}
                              <option value="">No epochs available</option>
                            {:else}
                              <option value="">All available data</option>
                              {#each options as option}
                                <option
                                  value={String(option.stream_epoch)}
                                  disabled={!option.selectable}
                                >
                                  {formatEarliestEpochOption(
                                    option,
                                  )}{option.selectable ? "" : " (unavailable)"}
                                </option>
                              {/each}
                            {/if}
                          </select>
                        </label>
                      </div>
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

  <!-- Modals & Dialogs -->
  <ConfirmDialog
    open={confirmCutoverOpen}
    title="Advance Starts to Finish"
    message={`Advance ${cutoverCandidates.length} Start/Finish reader(s) to Finish mode and start a new epoch "${cutoverEpochInput}"?`}
    confirmLabel="Advance to Finish"
    variant="warn"
    busy={cutoverBusy}
    onConfirm={handleCutoverConfirm}
    onCancel={() => {
      confirmCutoverOpen = false;
    }}
  />

  <ConfirmDialog
    open={confirmReplayAllOpen}
    title="Replay All Reads"
    message="Replay all retained reads from forwarders for all subscribed streams? The receiver will reset local data and cursors to 0. Existing DBF files will not be deleted."
    confirmLabel="Replay All Reads"
    variant="warn"
    busy={replayAllBusy}
    onConfirm={handleReplayAll}
    onCancel={() => {
      confirmReplayAllOpen = false;
    }}
  />

  <ConfirmDialog
    open={confirmingReplayStream !== null}
    title="Replay Reads"
    message={`Replay all retained reads for ${confirmingReplayStream ? streamPrimaryLabel(confirmingReplayStream) : "this stream"} from the forwarder? Local stream data and cursor will be reset to 0.`}
    confirmLabel="Replay Reads"
    variant="warn"
    busy={replayingStreamKey !== null}
    onConfirm={() => {
      if (confirmingReplayStream)
        void handleReplayStream(confirmingReplayStream);
    }}
    onCancel={() => {
      confirmingReplayStream = null;
    }}
  />

  <ConfirmDialog
    open={confirmingClearBothStream !== null}
    title="Clear Reads (Clean State)"
    message={`Purge all stored reads from the forwarder AND reset local receiver data for ${confirmingClearBothStream ? streamPrimaryLabel(confirmingClearBothStream) : "this stream"}? A new epoch will be started on the forwarder. This cannot be undone.`}
    confirmLabel="Clear Reads"
    variant="err"
    busy={clearingBothStreamKey !== null}
    onConfirm={() => {
      if (confirmingClearBothStream)
        void handleClearBoth(confirmingClearBothStream);
    }}
    onCancel={() => {
      confirmingClearBothStream = null;
    }}
  />

  {#if confirmBulkEpochOpen}
    <div
      class="fixed inset-0 bg-black/60 backdrop-blur-sm z-50 flex items-center justify-center p-4"
    >
      <div
        class="bg-surface-0 border border-border rounded-xl shadow-2xl max-w-sm w-full p-4 space-y-3"
      >
        <h3 class="text-sm font-bold text-text-primary">
          Advance Epoch for Selected Readers
        </h3>
        <p class="text-xs text-text-muted">
          Advancing the epoch creates a new clean timing window on the {selectedStreams.length}
          selected readers.
        </p>
        <label class="block text-xs font-medium text-text-secondary">
          New Epoch Name:
          <input
            type="text"
            bind:value={bulkEpochInput}
            placeholder="Finish Reads"
            class="mt-1 w-full px-2.5 py-1.5 rounded border border-border bg-surface-1 text-xs font-mono text-text-primary focus:outline-none focus:ring-1 focus:ring-accent"
          />
        </label>
        <div class="flex justify-end gap-2 pt-2">
          <button
            type="button"
            class={btnSecondary}
            class:!px-3={true}
            class:!py-1.5={true}
            class:!text-xs={true}
            onclick={() => {
              confirmBulkEpochOpen = false;
            }}
          >
            Cancel
          </button>
          <button
            type="button"
            class={btnPrimary}
            class:!px-3={true}
            class:!py-1.5={true}
            class:!text-xs={true}
            disabled={bulkEpochBusy}
            onclick={handleBulkAdvanceEpoch}
          >
            {bulkEpochBusy ? "Advancing…" : "Advance Epoch"}
          </button>
        </div>
      </div>
    </div>
  {/if}
</div>

<ExportTagdataModal
  open={exportModalOpen}
  initialStreamId={exportInitialStreamId}
  onclose={() => {
    exportModalOpen = false;
  }}
/>

<ForwarderConfigModal
  open={selectedConfigEndpointId !== null}
  endpointId={selectedConfigEndpointId}
  displayName={selectedConfigForwarder?.display_name ?? null}
  onClose={() => (selectedConfigEndpointId = null)}
/>

<ForwarderWifiModal
  open={selectedWifiEndpointId !== null}
  endpointId={selectedWifiEndpointId}
  displayName={selectedWifiForwarder?.display_name ?? null}
  onClose={() => (selectedWifiEndpointId = null)}
/>
