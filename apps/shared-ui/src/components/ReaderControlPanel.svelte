<script lang="ts">
  import AlertBanner from "./AlertBanner.svelte";
  import HelpTip from "./HelpTip.svelte";
  import {
    formatReadMode,
    formatTtoState,
    formatClockDrift,
    driftColorClass,
    computeDownloadPercent,
    formatLastSeen,
  } from "../lib/reader-view-model";
  import {
    READ_MODE_OPTIONS,
    shouldShowTimeoutInput,
    initialTimeoutDraft,
    resolveTimeoutSeconds,
  } from "../lib/read-mode-form";
  import type { HelpContextName } from "../lib/help/help-types";

  let {
    readerIp,
    readerInfo = null,
    readerState = "disconnected",
    downloadProgress = null,
    disabled = false,
    readerClockDisplay = undefined,
    localClockDisplay = undefined,
    lastRefreshDisplay = undefined,
    helpContext = "forwarder" as HelpContextName,
    onOpenHelpModal = undefined,
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
    readerInfo: any | null;
    readerState: string;
    downloadProgress: {
      state: string;
      reads_received: number;
      progress: number;
      total: number;
      error?: string;
    } | null;
    disabled: boolean;
    /** Pre-formatted reader clock string (ticking display managed by parent) */
    readerClockDisplay?: string;
    /** Pre-formatted local clock string */
    localClockDisplay?: string;
    /** Pre-formatted "last refresh" string (e.g. "5s ago") */
    lastRefreshDisplay?: string;
    /** Help context for HelpTip components */
    helpContext?: HelpContextName;
    /** Callback to open help modal for a given field key */
    onOpenHelpModal?: (fieldKey: string) => void;
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
  let feedback: { kind: "ok" | "warn" | "err"; message: string } | undefined =
    $state(undefined);
  let feedbackTimer: ReturnType<typeof setTimeout> | undefined;

  let readModeDraft: string | undefined = $state(undefined);
  let timeoutDraft: string | undefined = $state(undefined);

  let currentReadMode = $derived(
    readModeDraft ?? readerInfo?.config?.mode ?? "raw",
  );
  let currentTimeoutDraft = $derived(
    timeoutDraft ?? initialTimeoutDraft(readerInfo?.config?.timeout),
  );
  let showTimeout = $derived(shouldShowTimeoutInput(currentReadMode));

  function setFeedback(fb: { kind: "ok" | "err"; message: string }) {
    feedback = fb;
    clearTimeout(feedbackTimer);
    feedbackTimer = setTimeout(() => {
      feedback = undefined;
    }, 3000);
  }

  function clearFeedback() {
    clearTimeout(feedbackTimer);
    feedback = undefined;
  }

  async function wrap(fn: () => Promise<void>) {
    busy = true;
    clearFeedback();
    try {
      await fn();
    } catch (e: any) {
      const msg = typeof e === "string" ? e : e?.message ?? "Action failed";
      setFeedback({ kind: "err", message: msg });
    } finally {
      busy = false;
    }
  }

  async function handleSyncClock() {
    await wrap(async () => {
      await onSyncClock();
      setFeedback({ kind: "ok", message: "Clock synced" });
    });
  }

  async function handleSetReadMode() {
    const mode = currentReadMode;
    const timeout = resolveTimeoutSeconds(
      currentTimeoutDraft,
      readerInfo?.config?.timeout,
    );
    await wrap(async () => {
      await onSetReadMode(mode, timeout);
      readModeDraft = mode;
      timeoutDraft = String(timeout);
      setFeedback({
        kind: "ok",
        message: shouldShowTimeoutInput(mode)
          ? `Mode set to ${formatReadMode(mode)} (${timeout}s)`
          : `Mode set to ${formatReadMode(mode)}`,
      });
    });
  }

  async function handleSetTto() {
    const currentlyEnabled = readerInfo?.tto_enabled === true;
    await wrap(async () => {
      await onSetTto(!currentlyEnabled);
      setFeedback({
        kind: "ok",
        message: currentlyEnabled
          ? "TTO reporting disabled"
          : "TTO reporting enabled",
      });
    });
  }

  async function handleSetRecording() {
    const currentlyRecording = readerInfo?.recording === true;
    await wrap(async () => {
      await onSetRecording(!currentlyRecording);
      setFeedback({
        kind: "ok",
        message: currentlyRecording
          ? "Recording stopped"
          : "Recording started",
      });
    });
  }

  async function handleRefresh() {
    await wrap(async () => {
      await onRefresh();
      setFeedback({ kind: "ok", message: "Reader info refreshed" });
    });
  }

  async function handleClearRecords() {
    await wrap(async () => {
      await onClearRecords();
      setFeedback({ kind: "ok", message: "Clear records requested" });
    });
  }

  async function handleStartDownload() {
    await wrap(async () => {
      await onStartDownload();
      setFeedback({ kind: "ok", message: "Download started" });
    });
  }

  async function handleStopDownload() {
    if (!onStopDownload) return;
    await wrap(async () => {
      await onStopDownload!();
      setFeedback({ kind: "ok", message: "Download stopped" });
    });
  }

  async function handleReconnect() {
    await wrap(async () => {
      await onReconnect();
      setFeedback({ kind: "ok", message: "Reconnect requested" });
    });
  }

  let isDisabled = $derived(disabled || busy);

  let downloadPercent = $derived(
    computeDownloadPercent(
      downloadProgress,
      readerInfo?.estimated_stored_reads,
    ),
  );

  function openHelp(fieldKey: string) {
    onOpenHelpModal?.(fieldKey);
  }
</script>

<div class="mt-4 pt-4 border-t border-border">
  {#if !readerInfo && readerState === "disconnected"}
    <p class="text-sm text-text-muted">No reader data available</p>
  {:else}
    <!-- Info grid -->
    <div class="grid grid-cols-2 gap-x-8 gap-y-2 text-sm mb-4">
      <div class="col-span-2">
        <span class="text-text-muted">Banner:</span>
        <span class="font-mono ml-2 text-xs"
          >{readerInfo?.banner ?? "\u2014"}</span
        >
      </div>
      <div>
        <span class="text-text-muted">Firmware:</span>
        <span class="font-mono ml-2"
          >{readerInfo?.hardware?.fw_version ?? "\u2014"}</span
        >
      </div>
      <div>
        <span class="text-text-muted">Hardware:</span>
        <span class="font-mono ml-2"
          >{readerInfo?.hardware?.hw_code ?? "\u2014"}</span
        >
      </div>
      {#if readerClockDisplay !== undefined}
        <div>
          <span class="text-text-muted">Reader Clock:</span>
          <span class="font-mono ml-2">{readerClockDisplay}</span>
        </div>
      {/if}
      <div>
        <span class="text-text-muted"
          >Clock Drift:{#if onOpenHelpModal}<HelpTip
              fieldKey="clock_drift"
              sectionKey="reader_live"
              context={helpContext}
              onOpenModal={openHelp}
            />{/if}</span
        >
        <span
          class="{driftColorClass(readerInfo?.clock?.drift_ms)} font-mono ml-2"
          >{formatClockDrift(readerInfo?.clock?.drift_ms)}</span
        >
      </div>
      {#if localClockDisplay !== undefined}
        <div>
          <span class="text-text-muted">Local Clock:</span>
          <span class="font-mono ml-2">{localClockDisplay}</span>
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
    <div class="col-span-2 mb-4">
      <span class="text-sm text-text-muted"
        >Read Mode:{#if onOpenHelpModal}
          <HelpTip
            fieldKey="read_mode"
            sectionKey="read_mode"
            context={helpContext}
            onOpenModal={openHelp}
          />{/if}</span
      >
      <span class="ml-2 inline-flex items-center gap-2 flex-wrap">
        <select
          class="px-2 py-0.5 text-sm rounded-md bg-surface-0 text-text-primary border border-border"
          value={currentReadMode}
          onchange={(e) => {
            const mode = (e.currentTarget as HTMLSelectElement).value;
            readModeDraft = mode;
            if (
              shouldShowTimeoutInput(mode) &&
              timeoutDraft == null
            ) {
              timeoutDraft = initialTimeoutDraft(
                readerInfo?.config?.timeout,
              );
            }
          }}
          disabled={isDisabled}
        >
          {#each READ_MODE_OPTIONS as option}
            <option value={option.value}>{option.label}</option>
          {/each}
        </select>
        {#if showTimeout}
          <label
            class="inline-flex items-center gap-1 text-xs text-text-muted"
          >
            <span
              >Timeout{#if onOpenHelpModal}
                <HelpTip
                  fieldKey="timeout"
                  sectionKey="read_mode"
                  context={helpContext}
                  onOpenModal={openHelp}
                />{/if}</span
            >
            <input
              class="w-16 px-2 py-0.5 text-sm rounded-md bg-surface-0 text-text-primary border border-border"
              type="number"
              min="1"
              max="255"
              value={currentTimeoutDraft}
              oninput={(e) => {
                timeoutDraft = (e.currentTarget as HTMLInputElement).value;
              }}
              disabled={isDisabled}
            />
            <span>s</span>
          </label>
        {/if}
        <button
          class="px-2.5 py-0.5 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
          onclick={handleSetReadMode}
          disabled={isDisabled}>Apply</button
        >
      </span>
    </div>

    <!-- TTO toggle -->
    <div class="mb-4">
      <span class="text-sm text-text-muted"
        >TTO Bytes:{#if onOpenHelpModal}<HelpTip
            fieldKey="tto_bytes"
            sectionKey="reader_live"
            context={helpContext}
            onOpenModal={openHelp}
          />{/if}</span
      >
      <span class="ml-2 inline-flex items-center gap-2 flex-wrap">
        <span class="font-mono text-sm"
          >{formatTtoState(readerInfo?.tto_enabled)}</span
        >
        <button
          class="px-2.5 py-0.5 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
          onclick={handleSetTto}
          disabled={isDisabled}
        >
          {readerInfo?.tto_enabled ? "Disable TTO" : "Enable TTO"}
        </button>
      </span>
    </div>

    <!-- Action buttons row -->
    <div
      class="flex items-center gap-3 pt-3 border-t border-border flex-wrap"
    >
      <span class="inline-flex items-center gap-1">
        <button
          class="px-3 py-1.5 text-sm font-medium rounded-md text-white bg-accent border-none cursor-pointer hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed"
          onclick={handleSyncClock}
          disabled={isDisabled}>Sync Clock</button
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
          disabled={isDisabled}>Refresh</button
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
            ? "px-3 py-1.5 text-sm rounded-md bg-red-600 text-white border-none cursor-pointer hover:bg-red-700 disabled:opacity-50"
            : "px-3 py-1.5 text-sm rounded-md bg-green-600 text-white border-none cursor-pointer hover:bg-green-700 disabled:opacity-50"}
          onclick={handleSetRecording}
          disabled={isDisabled}
          >{readerInfo?.recording
            ? "Stop Recording"
            : "Start Recording"}</button
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
          disabled={isDisabled}>Download Reads</button
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
          disabled={isDisabled}>Clear Records</button
        >{#if onOpenHelpModal}<HelpTip
            fieldKey="clear_records"
            sectionKey="reader_live"
            context={helpContext}
            onOpenModal={openHelp}
          />{/if}
      </span>
      {#if onStopDownload && downloadProgress?.state === "downloading"}
        <button
          class="px-3 py-1.5 text-sm rounded-md bg-red-600 text-white border-none cursor-pointer hover:bg-red-700 disabled:opacity-50"
          onclick={handleStopDownload}
          disabled={isDisabled}>Stop Download</button
        >
      {/if}
      {#if readerState === "disconnected"}
        <button
          class="px-3 py-1.5 text-sm rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
          onclick={handleReconnect}
          disabled={isDisabled}>Reconnect</button
        >
      {/if}
    </div>

    <!-- Download progress bar -->
    {#if downloadProgress?.state === "downloading"}
      <div
        class="mt-3 flex items-center gap-3 text-sm text-text-secondary"
      >
        <div
          class="flex-1 h-2 rounded-full bg-surface-2 overflow-hidden"
        >
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
  {/if}
</div>
