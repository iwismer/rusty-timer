<script lang="ts">
  import AlertBanner from './AlertBanner.svelte';
  import HelpTip from './HelpTip.svelte';
  import {
    formatReadMode,
    formatTtoState,
    formatClockDrift,
    formatHardwareCode,
    formatEpochCreatedAt,
    formatEpochName,
    normalizeEpochNameDraft,
    advanceEpochWithOptionalName,
    driftColorClass,
    computeDownloadPercent,
    readerControlDisabled,
  } from '../lib/reader-view-model';
  import {
    READ_MODE_OPTIONS,
    shouldShowTimeoutInput,
    initialTimeoutDraft,
    resolveTimeoutSeconds,
  } from '../lib/read-mode-form';
  import type { HelpContextName } from '../lib/help/help-types';

  /** Reader info matching the Rust ReaderInfo struct. */
  export interface ReaderInfoData {
    banner?: string | null;
    hardware?: {
      fw_version?: string | null;
      hw_code?: string | number | null;
      reader_id?: string | number | null;
    } | null;
    config?: { mode: string; timeout: number } | null;
    tto_enabled?: boolean | null;
    clock?: { reader_clock: string; drift_ms: number } | null;
    estimated_stored_reads?: number | null;
    recording?: boolean | null;
    connect_failures?: number;
  }

  export type ReaderConnectionState = 'connected' | 'connecting' | 'disconnected';
  export type DownloadStateType = 'downloading' | 'complete' | 'error' | 'idle';

  let {
    readerIp,
    readerDetail = undefined,
    readerInfo = null,
    readerState = 'disconnected',
    readerStateLabel = undefined,
    showHeader = true,
    readsSession = null,
    readsTotal = null,
    lastSeenDisplay = undefined,
    localPortLabel = 'Local port',
    localPortValue = undefined,
    currentEpoch = null,
    currentEpochCreatedUnixMs = null,
    currentEpochName = null,
    epochEditable = true,
    epochBusy = false,
    detailsCollapsible = false,
    defaultCollapsed = false,
    downloadProgress = null,
    disabled = false,
    readerClockDisplay = undefined,
    forwarderClockDisplay = undefined,
    lastRefreshDisplay = undefined,
    helpContext = 'forwarder' as HelpContextName,
    onOpenHelpModal = undefined,
    onSetEpochName = undefined,
    onAdvanceEpoch = undefined,
    onSyncClock,
    onSetReadMode,
    onSetTto,
    onSetRecording,
    onClearRecords,
    onStartDownload,
    onStopDownload = undefined,
    onRefresh,
    onReconnect,
  }: {
    readerIp: string;
    /** Secondary identity, usually the stream id, when the display label may not be unique. */
    readerDetail?: string;
    readerInfo: ReaderInfoData | null;
    readerState: ReaderConnectionState;
    /** Raw state label from the owning app, e.g. "online" or "offline". */
    readerStateLabel?: string;
    /** Render the panel's own header row (reader label + state pill). */
    showHeader?: boolean;
    /** Reads seen this session; row hidden when null. */
    readsSession?: number | null;
    /** Total reads seen; row hidden when null. */
    readsTotal?: number | null;
    /** Pre-formatted "last seen" string (e.g. "5s ago"); hidden when undefined. */
    lastSeenDisplay?: string;
    /** Label for the local port entry, e.g. "Local Port" or "Local proxy". */
    localPortLabel?: string;
    /** Pre-formatted local port value; entry hidden when undefined. */
    localPortValue?: string;
    /** Numeric id of the currently active epoch, when known. */
    currentEpoch?: number | null;
    /** Unix-ms timestamp when the current epoch was created, when known. */
    currentEpochCreatedUnixMs?: number | null;
    /** Name of the currently active epoch, when known. */
    currentEpochName?: string | null;
    /** Whether the epoch name input/save controls are enabled. */
    epochEditable?: boolean;
    /** External busy flag for epoch controls. */
    epochBusy?: boolean;
    /** Show a Details toggle that collapses the detail grid and controls. */
    detailsCollapsible?: boolean;
    /** Initial collapsed state when detailsCollapsible is set. */
    defaultCollapsed?: boolean;
    downloadProgress: {
      state: DownloadStateType;
      reads_received: number;
      progress: number;
      total: number;
      error?: string;
    } | null;
    disabled: boolean;
    /** Pre-formatted reader clock string (ticking display managed by parent) */
    readerClockDisplay?: string;
    /** Pre-formatted forwarder-host clock string (ticking display managed by parent) */
    forwarderClockDisplay?: string;
    /** Pre-formatted "last refresh" string (e.g. "5s ago") */
    lastRefreshDisplay?: string;
    /** Help context for HelpTip components */
    helpContext?: HelpContextName;
    /** Callback to open help modal for a given field key */
    onOpenHelpModal?: (fieldKey: string) => void;
    /** Set (or clear, with null) the current epoch name. Epoch row renders when provided. */
    onSetEpochName?: (name: string | null) => Promise<void>;
    /** Advance to the next epoch. Epoch row renders when provided. */
    onAdvanceEpoch?: () => Promise<void>;
    onSyncClock: () => Promise<void>;
    onSetReadMode: (mode: string, timeout: number) => Promise<void>;
    onSetTto: (enabled: boolean) => Promise<void>;
    onSetRecording: (enabled: boolean) => Promise<void>;
    onClearRecords: () => Promise<void>;
    onStartDownload: () => Promise<void>;
    onStopDownload?: () => Promise<void>;
    onRefresh: () => Promise<void>;
    onReconnect: () => Promise<void>;
  } = $props();

  // --- Local UI state ---
  let busy = $state(false);
  let feedback: { kind: 'ok' | 'warn' | 'err'; message: string } | undefined = $state(undefined);
  let feedbackTimer: ReturnType<typeof setTimeout> | undefined;

  let readModeDraft: string | undefined = $state(undefined);
  let timeoutDraft: string | undefined = $state(undefined);
  let epochNameDraft = $state('');
  // svelte-ignore state_referenced_locally -- intentionally captures only the
  // initial collapsed state; the user owns the toggle afterwards.
  let detailsOpen = $state(!(detailsCollapsible && defaultCollapsed));

  let detailsShown = $derived(!detailsCollapsible || detailsOpen);

  let currentReadMode = $derived(readModeDraft ?? readerInfo?.config?.mode ?? 'raw');
  let currentTimeoutDraft = $derived(
    timeoutDraft ?? initialTimeoutDraft(readerInfo?.config?.timeout)
  );
  let showTimeout = $derived(shouldShowTimeoutInput(currentReadMode));

  let showEpochRow = $derived(onSetEpochName !== undefined || onAdvanceEpoch !== undefined);
  let showSummaryRow = $derived(
    readsSession != null ||
      readsTotal != null ||
      localPortValue !== undefined ||
      lastSeenDisplay !== undefined ||
      detailsCollapsible
  );

  function setFeedback(fb: { kind: 'ok' | 'err'; message: string }) {
    feedback = fb;
    clearTimeout(feedbackTimer);
    // Errors persist longer so the user has time to read them
    const timeout = fb.kind === 'err' ? 8000 : 3000;
    feedbackTimer = setTimeout(() => {
      feedback = undefined;
    }, timeout);
  }

  function clearFeedback() {
    clearTimeout(feedbackTimer);
    feedback = undefined;
  }

  async function wrap(fn: () => Promise<void>, actionName?: string) {
    busy = true;
    clearFeedback();
    try {
      await fn();
    } catch (e: any) {
      console.error(`ReaderControlPanel action failed${actionName ? ` (${actionName})` : ''}:`, e);
      const detail = typeof e === 'string' ? e : (e?.message ?? 'Unknown error');
      const msg = actionName ? `${actionName} failed: ${detail}` : detail;
      setFeedback({ kind: 'err', message: msg });
    } finally {
      busy = false;
    }
  }

  async function handleSyncClock() {
    await wrap(async () => {
      await onSyncClock();
      setFeedback({ kind: 'ok', message: 'Clock synced' });
    }, 'Sync Clock');
  }

  async function handleSetReadMode() {
    const mode = currentReadMode;
    const timeout = resolveTimeoutSeconds(currentTimeoutDraft, readerInfo?.config?.timeout);
    await wrap(async () => {
      await onSetReadMode(mode, timeout);
      readModeDraft = mode;
      timeoutDraft = String(timeout);
      setFeedback({
        kind: 'ok',
        message: shouldShowTimeoutInput(mode)
          ? `Mode set to ${formatReadMode(mode)} (${timeout}s)`
          : `Mode set to ${formatReadMode(mode)}`,
      });
    }, 'Set Read Mode');
  }

  async function handleSetTto() {
    const currentlyEnabled = readerInfo?.tto_enabled === true;
    await wrap(async () => {
      await onSetTto(!currentlyEnabled);
      setFeedback({
        kind: 'ok',
        message: currentlyEnabled ? 'TTO reporting disabled' : 'TTO reporting enabled',
      });
    }, 'Toggle TTO');
  }

  async function handleSetRecording() {
    const currentlyRecording = readerInfo?.recording === true;
    await wrap(async () => {
      await onSetRecording(!currentlyRecording);
      setFeedback({
        kind: 'ok',
        message: currentlyRecording ? 'Recording stopped' : 'Recording started',
      });
    }, 'Toggle Recording');
  }

  async function handleRefresh() {
    await wrap(async () => {
      await onRefresh();
      setFeedback({ kind: 'ok', message: 'Reader info refreshed' });
    }, 'Refresh');
  }

  async function handleClearRecords() {
    await wrap(async () => {
      await onClearRecords();
      setFeedback({ kind: 'ok', message: 'Clear records requested' });
    }, 'Clear Records');
  }

  async function handleStartDownload() {
    await wrap(async () => {
      await onStartDownload();
      setFeedback({ kind: 'ok', message: 'Download started' });
    }, 'Start Download');
  }

  async function handleStopDownload() {
    if (!onStopDownload) return;
    await wrap(async () => {
      await onStopDownload!();
      setFeedback({ kind: 'ok', message: 'Download stopped' });
    }, 'Stop Download');
  }

  async function handleReconnect() {
    await wrap(async () => {
      await onReconnect();
      setFeedback({ kind: 'ok', message: 'Reconnect requested' });
    }, 'Reconnect');
  }

  async function handleSaveEpochName() {
    if (!onSetEpochName) return;
    const name = normalizeEpochNameDraft(epochNameDraft);
    await wrap(async () => {
      await onSetEpochName!(name);
      if (name === null) epochNameDraft = '';
      setFeedback({
        kind: 'ok',
        message: name === null ? 'Epoch name cleared' : 'Epoch name saved',
      });
    }, 'Save Epoch Name');
  }

  async function handleAdvanceEpoch() {
    if (!onAdvanceEpoch) return;
    const draft = epochNameDraft;
    await wrap(async () => {
      const result = await advanceEpochWithOptionalName(draft, onAdvanceEpoch!, onSetEpochName);
      setFeedback({
        kind: 'ok',
        message:
          result === 'advanced'
            ? 'Advanced to next epoch'
            : 'Advanced to next epoch and saved name',
      });
    }, 'Advance Epoch');
  }

  let isDisabled = $derived(disabled || busy);
  let controlDisabled = $derived(disabled || readerControlDisabled(readerState, busy));
  let epochControlDisabled = $derived(disabled || busy || epochBusy || !epochEditable);

  let downloadPercent = $derived(
    computeDownloadPercent(downloadProgress, readerInfo?.estimated_stored_reads)
  );

  function openHelp(fieldKey: string) {
    onOpenHelpModal?.(fieldKey);
  }
</script>

<div
  class={showHeader ? 'mt-4 pt-4 border-t border-border' : ''}
  data-testid="reader-control-panel"
>
  {#if showHeader}
    <div class="mb-3 flex flex-wrap items-start justify-between gap-2">
      <div class="min-w-0">
        <p class="truncate text-sm font-semibold text-text-primary">
          Reader: {readerIp}
        </p>
        {#if readerInfo?.hardware?.reader_id != null}
          <p class="mt-0.5 truncate font-mono text-xs text-text-muted">
            Hardware reader ID: {readerInfo.hardware.reader_id}
          </p>
        {/if}
        {#if readerDetail}
          <p class="mt-0.5 truncate font-mono text-xs text-text-muted">
            Stream: {readerDetail}
          </p>
        {/if}
      </div>
      <span
        class="rounded-full border border-border bg-surface-2 px-2 py-0.5 text-xs text-text-muted"
      >
        {readerStateLabel ?? readerState}
      </span>
    </div>
  {/if}

  <!-- Always-visible summary row -->
  {#if showSummaryRow}
    <div class="mb-3 flex items-start justify-between gap-4">
      <div class="flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
        {#if readsSession != null}
          <div class="inline-flex items-baseline gap-1">
            <span class="text-text-muted">Reads (session):</span>
            <span class="font-mono text-text-primary">{readsSession.toLocaleString()}</span>
            {#if onOpenHelpModal}<HelpTip
                fieldKey="reads_session"
                sectionKey="reader_live"
                context={helpContext}
                onOpenModal={openHelp}
              />{/if}
          </div>
        {/if}
        {#if readsTotal != null}
          <div class="inline-flex items-baseline gap-1">
            <span class="text-text-muted">Reads (total):</span>
            <span class="font-mono text-text-primary">{readsTotal.toLocaleString()}</span>
            {#if onOpenHelpModal}<HelpTip
                fieldKey="reads_total"
                sectionKey="reader_live"
                context={helpContext}
                onOpenModal={openHelp}
              />{/if}
          </div>
        {/if}
        {#if localPortValue !== undefined}
          <div class="inline-flex items-baseline gap-1">
            <span class="text-text-muted">{localPortLabel}:</span>
            <span class="font-mono text-text-primary">{localPortValue}</span>
            {#if onOpenHelpModal}<HelpTip
                fieldKey="local_port"
                sectionKey="reader_live"
                context={helpContext}
                onOpenModal={openHelp}
              />{/if}
          </div>
        {/if}
        {#if lastSeenDisplay !== undefined}
          <div class="inline-flex items-baseline gap-1">
            <span class="text-text-muted">Last seen:</span>
            <span class="text-text-secondary">{lastSeenDisplay}</span>
            {#if onOpenHelpModal}<HelpTip
                fieldKey="last_seen"
                sectionKey="reader_live"
                context={helpContext}
                onOpenModal={openHelp}
              />{/if}
          </div>
        {/if}
      </div>
      {#if detailsCollapsible}
        <button
          class="shrink-0 inline-flex items-center gap-1 px-2 py-1 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2"
          onclick={() => {
            detailsOpen = !detailsOpen;
          }}
          aria-expanded={detailsOpen}
          aria-label={detailsOpen ? 'Hide details' : 'Show details'}
        >
          <span class={`inline-block transition-transform ${detailsOpen ? 'rotate-180' : ''}`}
            >▾</span
          >
          <span>Details</span>
        </button>
      {/if}
    </div>
  {/if}

  <!-- Epoch name row -->
  {#if showEpochRow}
    <div class="mb-3 flex flex-col gap-1">
      {#if currentEpoch != null || currentEpochName || currentEpochCreatedUnixMs != null}
        <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-text-muted">
          <span class="inline-flex items-baseline gap-1">
            <span>Current epoch:</span>
            <span class="font-mono text-text-primary">
              {#if currentEpoch != null}#{currentEpoch}{:else}—{/if}
            </span>
            {#if onOpenHelpModal}<HelpTip
                fieldKey="current_epoch"
                sectionKey="reader_live"
                context={helpContext}
                onOpenModal={openHelp}
              />{/if}
          </span>
          <span>
            Name:
            <span class="font-mono text-text-primary">
              {formatEpochName(currentEpochName)}
            </span>
          </span>
          <span class="inline-flex items-baseline gap-1">
            <span>Created:</span>
            <span class="font-mono text-text-primary">
              {formatEpochCreatedAt(currentEpochCreatedUnixMs)}
            </span>
            {#if onOpenHelpModal}<HelpTip
                fieldKey="current_epoch_created"
                sectionKey="reader_live"
                context={helpContext}
                onOpenModal={openHelp}
              />{/if}
          </span>
        </div>
      {/if}
      <div class="flex items-center gap-2 flex-wrap">
        {#if onSetEpochName}
          <span class="text-xs text-text-muted">New Epoch Name:</span>
          <input
            type="text"
            class="w-48 px-2 py-1 text-xs rounded-md bg-surface-0 text-text-primary border border-border"
            placeholder="Set epoch name"
            bind:value={epochNameDraft}
            disabled={epochControlDisabled}
          />
          {#if onOpenHelpModal}<HelpTip
              fieldKey="epoch_name"
              sectionKey="reader_live"
              context={helpContext}
              onOpenModal={openHelp}
            />{/if}
          <button
            onclick={handleSaveEpochName}
            class="px-2 py-1 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50 disabled:cursor-not-allowed"
            disabled={epochControlDisabled}
          >
            Save
          </button>
        {/if}
        {#if onAdvanceEpoch}
          <button
            onclick={handleAdvanceEpoch}
            class="px-2 py-1 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50 disabled:cursor-not-allowed"
            disabled={epochControlDisabled}
          >
            Advance Epoch
          </button>
          {#if onOpenHelpModal}<HelpTip
              fieldKey="advance_epoch"
              sectionKey="reader_live"
              context={helpContext}
              onOpenModal={openHelp}
            />{/if}
        {/if}
      </div>
    </div>
  {/if}

  {#if detailsShown}
    <div class={detailsCollapsible ? 'mt-4 pt-4 border-t border-border' : ''}>
      {#if !readerInfo && readerState === 'disconnected'}
        <p class="mb-4 text-sm text-text-muted">No reader data available</p>
      {/if}

      <!-- Info grid -->
      <div class="grid grid-cols-2 gap-x-8 gap-y-2 text-sm mb-4">
        <div class="col-span-2">
          <span class="text-text-muted">Banner:</span>
          <span class="font-mono ml-2 text-xs">{readerInfo?.banner ?? '\u2014'}</span>
        </div>
        <div>
          <span class="text-text-muted">Firmware:</span>
          <span class="font-mono ml-2">{readerInfo?.hardware?.fw_version ?? '\u2014'}</span>
        </div>
        <div>
          <span class="text-text-muted">Hardware:</span>
          <span class="font-mono ml-2">{formatHardwareCode(readerInfo?.hardware?.hw_code)}</span>
        </div>
        {#if readerClockDisplay !== undefined}
          <div>
            <span class="text-text-muted">Reader Clock:</span>
            <span class="font-mono ml-2">{readerClockDisplay}</span>
          </div>
        {/if}
        <div class="inline-flex items-baseline gap-1">
          <span class="text-text-muted">Clock Drift:</span>
          <span class="{driftColorClass(readerInfo?.clock?.drift_ms)} font-mono"
            >{formatClockDrift(readerInfo?.clock?.drift_ms)}</span
          >
          {#if onOpenHelpModal}<HelpTip
              fieldKey="clock_drift"
              sectionKey="reader_live"
              context={helpContext}
              onOpenModal={openHelp}
            />{/if}
        </div>
        {#if forwarderClockDisplay !== undefined}
          <div>
            <span class="text-text-muted">Forwarder Clock:</span>
            <span class="font-mono ml-2">{forwarderClockDisplay}</span>
          </div>
        {/if}
        {#if lastRefreshDisplay !== undefined}
          <div>
            <span class="text-text-muted">Last Refresh:</span>
            <span class="ml-2">{lastRefreshDisplay}</span>
          </div>
        {/if}
      </div>

      <!-- Read mode controls -->
      <div class="col-span-2 mb-4 inline-flex items-center gap-2 flex-wrap">
        <span class="text-sm text-text-muted">Read Mode:</span>
        <span class="inline-flex items-center gap-2 flex-wrap">
          <select
            class="px-2 py-0.5 text-sm rounded-md bg-surface-0 text-text-primary border border-border"
            value={currentReadMode}
            onchange={(e) => {
              const mode = (e.currentTarget as HTMLSelectElement).value;
              readModeDraft = mode;
              if (shouldShowTimeoutInput(mode) && timeoutDraft == null) {
                timeoutDraft = initialTimeoutDraft(readerInfo?.config?.timeout);
              }
            }}
            disabled={controlDisabled}
          >
            {#each READ_MODE_OPTIONS as option}
              <option value={option.value}>{option.label}</option>
            {/each}
          </select>
          {#if onOpenHelpModal}
            <HelpTip
              fieldKey="read_mode"
              sectionKey="read_mode"
              context={helpContext}
              onOpenModal={openHelp}
            />
          {/if}
          {#if showTimeout}
            <label class="inline-flex items-center gap-1 text-xs text-text-muted">
              <span>Timeout</span>
              <input
                class="w-16 px-2 py-0.5 text-sm rounded-md bg-surface-0 text-text-primary border border-border"
                type="number"
                min="1"
                max="255"
                value={currentTimeoutDraft}
                oninput={(e) => {
                  timeoutDraft = (e.currentTarget as HTMLInputElement).value;
                }}
                disabled={controlDisabled}
              />
              <span>s</span>
              {#if onOpenHelpModal}
                <HelpTip
                  fieldKey="timeout"
                  sectionKey="read_mode"
                  context={helpContext}
                  onOpenModal={openHelp}
                />
              {/if}
            </label>
          {/if}
          <button
            class="px-2.5 py-0.5 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
            onclick={handleSetReadMode}
            disabled={controlDisabled}>Apply</button
          >
        </span>
      </div>

      <!-- TTO toggle -->
      <div class="mb-4 inline-flex items-center gap-2 flex-wrap">
        <span class="text-sm text-text-muted">TTO Bytes:</span>
        <span class="inline-flex items-center gap-2 flex-wrap">
          <span class="font-mono text-sm">{formatTtoState(readerInfo?.tto_enabled)}</span>
          {#if onOpenHelpModal}<HelpTip
              fieldKey="tto_bytes"
              sectionKey="reader_live"
              context={helpContext}
              onOpenModal={openHelp}
            />{/if}
          <button
            class="px-2.5 py-0.5 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
            onclick={handleSetTto}
            disabled={controlDisabled}
          >
            {readerInfo?.tto_enabled ? 'Disable TTO' : 'Enable TTO'}
          </button>
        </span>
      </div>

      <!-- Action buttons row -->
      <div class="flex items-center gap-3 pt-3 border-t border-border flex-wrap">
        <span class="inline-flex items-center gap-1">
          <button
            class="px-3 py-1.5 text-sm font-medium rounded-md text-white bg-accent border-none cursor-pointer hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed"
            onclick={handleSyncClock}
            disabled={controlDisabled}>Sync Clock</button
          >{#if onOpenHelpModal}<HelpTip
              fieldKey="sync_clock"
              sectionKey="reader_live"
              context={helpContext}
              onOpenModal={openHelp}
            />{/if}
        </span>
        <span class="inline-flex items-center gap-1">
          <button
            class="px-3 py-1.5 text-sm rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
            onclick={handleRefresh}
            disabled={controlDisabled}>Refresh</button
          >{#if onOpenHelpModal}<HelpTip
              fieldKey="refresh_reader"
              sectionKey="reader_live"
              context={helpContext}
              onOpenModal={openHelp}
            />{/if}
        </span>
        <span class="inline-flex items-center gap-1">
          <button
            class={readerInfo?.recording
              ? 'px-3 py-1.5 text-sm rounded-md bg-red-600 text-white border-none cursor-pointer hover:bg-red-700 disabled:opacity-50'
              : 'px-3 py-1.5 text-sm rounded-md bg-green-600 text-white border-none cursor-pointer hover:bg-green-700 disabled:opacity-50'}
            onclick={handleSetRecording}
            disabled={controlDisabled}
            >{readerInfo?.recording ? 'Stop Recording' : 'Start Recording'}</button
          >{#if onOpenHelpModal}<HelpTip
              fieldKey="recording"
              sectionKey="reader_live"
              context={helpContext}
              onOpenModal={openHelp}
            />{/if}
        </span>
        <span class="inline-flex items-center gap-1">
          <button
            class="px-3 py-1.5 text-sm font-medium rounded-md text-white bg-accent border-none cursor-pointer hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed"
            onclick={handleStartDownload}
            disabled={controlDisabled}>Download Reads</button
          >{#if onOpenHelpModal}<HelpTip
              fieldKey="download_reads"
              sectionKey="reader_live"
              context={helpContext}
              onOpenModal={openHelp}
            />{/if}
        </span>
        <span class="inline-flex items-center gap-1">
          <button
            class="px-3 py-1.5 text-sm rounded-md bg-red-600 text-white border-none cursor-pointer hover:bg-red-700 disabled:opacity-50"
            onclick={handleClearRecords}
            disabled={controlDisabled}>Clear Records</button
          >{#if onOpenHelpModal}<HelpTip
              fieldKey="clear_records"
              sectionKey="reader_live"
              context={helpContext}
              onOpenModal={openHelp}
            />{/if}
        </span>
        {#if onStopDownload && downloadProgress?.state === 'downloading'}
          <button
            class="px-3 py-1.5 text-sm rounded-md bg-red-600 text-white border-none cursor-pointer hover:bg-red-700 disabled:opacity-50"
            onclick={handleStopDownload}
            disabled={controlDisabled}>Stop Download</button
          >
        {/if}
        {#if readerState === 'disconnected'}
          <button
            class="px-3 py-1.5 text-sm rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
            onclick={handleReconnect}
            disabled={isDisabled}>Reconnect</button
          >
        {/if}
      </div>
    </div>
  {/if}

  <!-- Download progress bar -->
  {#if downloadProgress?.state === 'downloading'}
    <div class="mt-3 flex items-center gap-3 text-sm text-text-secondary">
      <div class="flex-1 h-2 rounded-full bg-surface-2 overflow-hidden">
        <div
          class="h-full bg-accent rounded-full transition-all"
          style="width: {downloadPercent}%"
        ></div>
      </div>
      <span class="text-xs font-mono whitespace-nowrap">
        {downloadProgress.reads_received} reads &middot; {downloadPercent}%
      </span>
    </div>
  {/if}

  <!-- Feedback banner -->
  {#if feedback}
    <div class="mt-3">
      <AlertBanner
        variant={feedback.kind}
        message={feedback.message}
        onDismiss={() => {
          clearFeedback();
        }}
      />
    </div>
  {/if}
</div>
