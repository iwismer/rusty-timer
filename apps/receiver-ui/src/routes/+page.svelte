<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import * as api from "$lib/api";
  import { buildUpdatedSubscriptions } from "$lib/subscriptions";
  import { initSSE, destroySSE } from "$lib/sse";
  import { waitForApplyResult } from "@rusty-timer/shared-ui/lib/update-flow";
  import {
    UpdateBanner,
    LogViewer,
    Card,
    StatusBadge,
    AlertBanner,
  } from "@rusty-timer/shared-ui";
  import type {
    EpochScope,
    RaceEntry,
    ReceiverSelection,
    ReplayPolicy,
    Profile,
    StreamCountUpdate,
    StatusResponse,
    StreamsResponse,
    LogsResponse,
    ReplayTargetEpochOption,
  } from "$lib/api";
  type TargetedRowDraft = {
    streamKey: string;
    streamEpoch: string;
  };
  type TargetedRowErrors = {
    streamKey?: string;
    streamEpoch?: string;
  };

  let profile = $state<Profile | null>(null);
  let status = $state<StatusResponse | null>(null);
  let streams = $state<StreamsResponse | null>(null);
  let logs = $state<LogsResponse | null>(null);
  let error = $state<string | null>(null);
  let epochLoadError = $state<string | null>(null);

  // Edit state
  let editServerUrl = $state("");
  let editToken = $state("");
  let editUpdateMode = $state("check-and-download");
  let checkingUpdate = $state(false);
  let checkMessage = $state<string | null>(null);
  let saving = $state(false);
  let connectBusy = $state(false);
  let sseConnected = $state(false);
  let updateVersion = $state<string | null>(null);
  let updateStatus = $state<"available" | "downloaded" | null>(null);
  let updateBusy = $state(false);
  let portOverrides = $state<Record<string, string | number | null>>({});
  let subscriptionsBusy = $state(false);
  let activeSubscriptionKey = $state<string | null>(null);
  let races = $state<RaceEntry[]>([]);
  let selectionMode = $state<ReceiverSelection["mode"]>("manual");
  let selectedStreams = $state<{ forwarder_id: string; reader_ip: string }[]>(
    [],
  );
  let raceIdDraft = $state("");
  let epochScopeDraft = $state<EpochScope>("current");
  let replayPolicyDraft = $state<ReplayPolicy>("resume");
  let targetedRows = $state<TargetedRowDraft[]>([
    { streamKey: "", streamEpoch: "" },
  ]);
  let targetedEpochOptionsByStream = $state<
    Record<string, ReplayTargetEpochOption[]>
  >({});
  let targetedRowErrors = $state<Record<number, TargetedRowErrors>>({});
  let selectionBusy = $state(false);
  let selectionApplyQueued = $state(false);
  let savedPayload = $state<string | null>(null);
  let isDirty = $derived(
    savedPayload !== null &&
      JSON.stringify(selectionPayload()) !== savedPayload,
  );
  let loadAllInFlight = false;
  let loadAllQueued = false;

  function normalizeReplayPolicy(replayPolicy: ReplayPolicy): ReplayPolicy {
    if (replayPolicy === "live_only") {
      return "live_only";
    }
    if (replayPolicy === "targeted") {
      return "targeted";
    }
    return "resume";
  }

  function rowFromReplayTarget(target: api.ReplayTarget): TargetedRowDraft {
    return {
      streamKey: streamKey(target.forwarder_id, target.reader_ip),
      streamEpoch: String(target.stream_epoch),
    };
  }

  function parseStreamKey(value: string): api.StreamRef | null {
    const separator = value.indexOf("/");
    if (separator <= 0 || separator === value.length - 1) {
      return null;
    }
    const forwarder_id = value.slice(0, separator).trim();
    const reader_ip = value.slice(separator + 1).trim();
    if (!forwarder_id || !reader_ip) {
      return null;
    }
    return { forwarder_id, reader_ip };
  }

  function parseNonNegativeInt(raw: string): number | null {
    const trimmed = raw.trim();
    if (!/^\d+$/.test(trimmed)) {
      return null;
    }
    const parsed = Number(trimmed);
    if (!Number.isSafeInteger(parsed) || parsed < 0) {
      return null;
    }
    return parsed;
  }

  function validateTargetedRows(): {
    replayTargets: api.ReplayTarget[];
    errors: Record<number, TargetedRowErrors>;
  } {
    const replayTargets: api.ReplayTarget[] = [];
    const errors: Record<number, TargetedRowErrors> = {};

    targetedRows.forEach((row, index) => {
      const rowErrors: TargetedRowErrors = {};
      const parsedStream = parseStreamKey(row.streamKey);
      if (!parsedStream) {
        rowErrors.streamKey = "Select a stream.";
      }

      const epoch = parseNonNegativeInt(row.streamEpoch);
      if (epoch === null) {
        rowErrors.streamEpoch = row.streamEpoch.trim()
          ? "Stream epoch must be a non-negative integer."
          : "Stream epoch is required.";
      }

      if (rowErrors.streamKey || rowErrors.streamEpoch) {
        errors[index] = rowErrors;
        return;
      }

      replayTargets.push({
        forwarder_id: parsedStream!.forwarder_id,
        reader_ip: parsedStream!.reader_ip,
        stream_epoch: epoch!,
      });
    });

    return { replayTargets, errors };
  }

  function streamKey(forwarder_id: string, reader_ip: string): string {
    return `${forwarder_id}/${reader_ip}`;
  }

  async function ensureTargetedEpochOptionsForStream(
    key: string,
  ): Promise<void> {
    if (!key || targetedEpochOptionsByStream[key] !== undefined) {
      return;
    }
    const parsed = parseStreamKey(key);
    if (!parsed) {
      return;
    }
    try {
      epochLoadError = null;
      const response = await api.getReplayTargetEpochs(parsed);
      targetedEpochOptionsByStream = {
        ...targetedEpochOptionsByStream,
        [key]: response.epochs,
      };
    } catch (e) {
      // Do not cache transient failures as empty options; allow in-session retry.
      epochLoadError = `Failed to load epoch options: ${String(e)}`;
    }
  }

  function epochFallbackLabel(option: ReplayTargetEpochOption): string {
    if (option.name && option.name.trim().length > 0) {
      return option.name.trim();
    }
    if (option.first_seen_at) {
      const firstSeen = new Date(option.first_seen_at);
      if (!Number.isNaN(firstSeen.getTime())) {
        return `Epoch ${option.stream_epoch} (${firstSeen.toLocaleString()})`;
      }
    }
    return `Epoch ${option.stream_epoch}`;
  }

  function targetedEpochOptionLabel(option: ReplayTargetEpochOption): string {
    const base = epochFallbackLabel(option);
    if (option.race_names.length === 0) {
      return base;
    }
    return `${base} - Race: ${option.race_names.join(", ")}`;
  }

  function targetedRowEpochOptions(
    row: TargetedRowDraft,
  ): ReplayTargetEpochOption[] {
    if (!row.streamKey) {
      return [];
    }
    return targetedEpochOptionsByStream[row.streamKey] ?? [];
  }

  function applyStreamCountUpdates(updates: StreamCountUpdate[]): boolean {
    if (updates.length === 0) {
      return false;
    }
    if (!streams) {
      return true;
    }

    const knownKeys = new Set(
      streams.streams.map((s) => streamKey(s.forwarder_id, s.reader_ip)),
    );
    const updatesByKey = new Map(
      updates.map((u) => [streamKey(u.forwarder_id, u.reader_ip), u]),
    );
    const hasUnknownStream = updates.some(
      (u) => !knownKeys.has(streamKey(u.forwarder_id, u.reader_ip)),
    );

    streams = {
      ...streams,
      streams: streams.streams.map((stream) => {
        if (!stream.subscribed) {
          return stream;
        }
        const update = updatesByKey.get(
          streamKey(stream.forwarder_id, stream.reader_ip),
        );
        if (!update) {
          return stream;
        }
        return {
          ...stream,
          reads_total: update.reads_total,
          reads_epoch: update.reads_epoch,
        };
      }),
    };

    return hasUnknownStream;
  }

  async function toggleSubscription(
    forwarder_id: string,
    reader_ip: string,
    currentlySubscribed: boolean,
  ) {
    if (subscriptionsBusy) {
      return;
    }

    error = null;
    const key = streamKey(forwarder_id, reader_ip);
    subscriptionsBusy = true;
    activeSubscriptionKey = key;
    try {
      const updated = buildUpdatedSubscriptions({
        allStreams: streams?.streams ?? [],
        target: {
          forwarder_id,
          reader_ip,
          currentlySubscribed,
        },
        rawPortOverride: portOverrides[key],
      });
      if (updated.error) {
        error = updated.error;
        return;
      }

      await api.putSubscriptions(updated.subscriptions ?? []);
      streams = await api.getStreams();
      if (!currentlySubscribed) {
        const { [key]: _, ...rest } = portOverrides;
        portOverrides = rest;
      }
    } catch (e) {
      error = String(e);
    } finally {
      subscriptionsBusy = false;
      activeSubscriptionKey = null;
    }
  }

  async function loadAll() {
    if (loadAllInFlight) {
      loadAllQueued = true;
      return;
    }

    loadAllInFlight = true;
    try {
      const [nextStatus, nextStreams, nextLogs, nextSelection, nextRaces] =
        await Promise.all([
          api.getStatus(),
          api.getStreams(),
          api.getLogs(),
          api.getSelection().catch(() => null),
          api.getRaces().catch(() => ({ races: [] })),
        ]);
      status = nextStatus;
      streams = nextStreams;
      logs = nextLogs;
      races = nextRaces.races;
      if (nextSelection) {
        selectionMode = nextSelection.selection.mode;
        replayPolicyDraft = normalizeReplayPolicy(nextSelection.replay_policy);
        targetedRows =
          nextSelection.replay_policy === "targeted" &&
          nextSelection.replay_targets &&
          nextSelection.replay_targets.length > 0
            ? nextSelection.replay_targets.map(rowFromReplayTarget)
            : [{ streamKey: "", streamEpoch: "" }];
        for (const row of targetedRows) {
          if (row.streamKey) {
            void ensureTargetedEpochOptionsForStream(row.streamKey);
          }
        }
        targetedRowErrors = {};
        if (nextSelection.selection.mode === "manual") {
          selectedStreams = nextSelection.selection.streams;
          raceIdDraft = "";
          epochScopeDraft = "current";
        } else {
          raceIdDraft = nextSelection.selection.race_id;
          epochScopeDraft = nextSelection.selection.epoch_scope;
          selectedStreams = [];
        }
        savedPayload = JSON.stringify(selectionPayload());
      }
      const p = await api.getProfile().catch(() => null);
      if (p) {
        profile = p;
        editServerUrl = p.server_url;
        editToken = p.token;
        editUpdateMode = p.update_mode || "check-and-download";
      }
      const us = await api.getUpdateStatus().catch(() => null);
      if (
        (us?.status === "downloaded" || us?.status === "available") &&
        us.version
      ) {
        updateVersion = us.version;
        updateStatus = us.status;
      } else {
        updateVersion = null;
        updateStatus = null;
      }
    } catch (e) {
      error = String(e);
    } finally {
      loadAllInFlight = false;
      if (loadAllQueued) {
        loadAllQueued = false;
        void loadAll();
      }
    }
  }

  function selectionPayload(): api.ReceiverSetSelection {
    const replay_targets =
      replayPolicyDraft === "targeted"
        ? validateTargetedRows().replayTargets
        : undefined;

    if (selectionMode === "manual") {
      return {
        selection: {
          mode: "manual",
          streams: selectedStreams,
        },
        replay_policy: replayPolicyDraft,
        ...(replayPolicyDraft === "targeted" ? { replay_targets } : {}),
      };
    }

    return {
      selection: {
        mode: "race",
        race_id: raceIdDraft.trim(),
        epoch_scope: epochScopeDraft,
      },
      replay_policy: replayPolicyDraft,
      ...(replayPolicyDraft === "targeted" ? { replay_targets } : {}),
    };
  }

  async function applySelection(): Promise<void> {
    selectionApplyQueued = true;
    if (selectionBusy) return;

    selectionBusy = true;
    error = null;
    while (selectionApplyQueued) {
      selectionApplyQueued = false;
      if (replayPolicyDraft === "targeted") {
        const validation = validateTargetedRows();
        targetedRowErrors = validation.errors;
        if (
          Object.keys(validation.errors).length > 0 ||
          validation.replayTargets.length === 0
        ) {
          if (validation.replayTargets.length === 0) {
            targetedRowErrors = {
              ...(Object.keys(validation.errors).length > 0
                ? validation.errors
                : { 0: {} }),
              0: {
                ...(validation.errors[0] ?? {}),
                streamKey:
                  validation.errors[0]?.streamKey ??
                  "Add at least one valid replay target.",
              },
            };
          }
          continue;
        }
      } else {
        targetedRowErrors = {};
      }
      try {
        const payload = selectionPayload();
        await api.putSelection(payload);
        savedPayload = JSON.stringify(payload);
        error = null;
      } catch (e) {
        error = String(e);
        if (!selectionApplyQueued) {
          break;
        }
      }
    }

    selectionBusy = false;
  }

  function handleSelectionModeChange(event: Event): void {
    const nextMode = (event.currentTarget as HTMLSelectElement).value as
      | "manual"
      | "race";
    selectionMode = nextMode;
  }

  function handleRaceIdChange(event: Event): void {
    raceIdDraft = (event.currentTarget as HTMLSelectElement).value;
  }

  function handleEpochScopeChange(event: Event): void {
    epochScopeDraft = (event.currentTarget as HTMLSelectElement)
      .value as EpochScope;
  }

  function handleReplayPolicyChange(event: Event): void {
    replayPolicyDraft = (event.currentTarget as HTMLSelectElement)
      .value as ReplayPolicy;
  }

  function handleTargetedStreamChange(index: number, event: Event): void {
    const value = (event.currentTarget as HTMLSelectElement).value;
    targetedRows = targetedRows.map((row, rowIndex) =>
      rowIndex === index ? { ...row, streamKey: value, streamEpoch: "" } : row,
    );
    if (value) {
      void ensureTargetedEpochOptionsForStream(value);
    }
  }

  function handleTargetedEpochChange(index: number, event: Event): void {
    const value = (event.currentTarget as HTMLSelectElement).value.trim();
    targetedRows = targetedRows.map((row, rowIndex) =>
      rowIndex === index ? { ...row, streamEpoch: value } : row,
    );
  }

  function addTargetedRow(): void {
    targetedRows = [...targetedRows, { streamKey: "", streamEpoch: "" }];
    targetedRowErrors = {};
  }

  function removeTargetedRow(index: number): void {
    targetedRows = targetedRows.filter((_, rowIndex) => rowIndex !== index);
    if (targetedRows.length === 0) {
      targetedRows = [{ streamKey: "", streamEpoch: "" }];
    }
    targetedRowErrors = {};
  }

  async function saveProfile() {
    saving = true;
    try {
      await api.putProfile({
        server_url: editServerUrl,
        token: editToken,
        update_mode: editUpdateMode,
      });
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }

  async function handleCheckUpdate() {
    checkingUpdate = true;
    checkMessage = null;
    try {
      const result = await api.checkForUpdate();
      if (result.status === "up_to_date") {
        checkMessage = "Up to date.";
      } else if (
        result.status === "available" ||
        result.status === "downloaded"
      ) {
        checkMessage = null; // UpdateBanner will show via SSE
        updateVersion = result.version ?? null;
        updateStatus = result.status;
      } else if (result.status === "failed") {
        checkMessage = result.error ?? "Update check failed.";
      }
    } catch (e) {
      checkMessage = String(e);
    } finally {
      checkingUpdate = false;
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

  async function handleConnect() {
    connectBusy = true;
    try {
      await api.connect();
    } catch (e) {
      error = String(e);
    } finally {
      connectBusy = false;
    }
  }

  async function handleDisconnect() {
    connectBusy = true;
    try {
      await api.disconnect();
    } catch (e) {
      error = String(e);
    } finally {
      connectBusy = false;
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

  onMount(() => {
    void loadAll();
    initSSE({
      onStatusChanged: (s) => {
        status = s;
      },
      onStreamsSnapshot: (s) => {
        streams = s;
      },
      onLogEntry: (entry) => {
        if (logs) {
          logs = { entries: [...logs.entries, entry] };
        } else {
          logs = { entries: [entry] };
        }
      },
      onResync: () => {
        loadAll();
      },
      onConnectionChange: (connected) => {
        sseConnected = connected;
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
      onStreamCountsUpdated: (updates) => {
        const needsResync = applyStreamCountUpdates(updates);
        if (needsResync) {
          void loadAll();
        }
      },
    });
  });

  onDestroy(() => {
    destroySSE();
  });

  let connectionState = $derived(status?.connection_state ?? "unknown");
  let connectionBadgeState = $derived(
    (connectionState === "connected"
      ? "ok"
      : connectionState === "disconnected"
        ? "err"
        : "warn") as "ok" | "warn" | "err",
  );
  let subscribedCount = $derived(
    streams?.streams.filter((s) => s.subscribed).length ?? 0,
  );
  let totalCount = $derived(streams?.streams.length ?? 0);

  const inputClass =
    "w-full px-3 py-1.5 text-sm rounded-md bg-surface-0 border border-border text-text-primary font-mono focus:outline-none focus:ring-1 focus:ring-accent";
  const btnPrimary =
    "px-3 py-1.5 text-sm font-medium rounded-md text-white bg-accent border-none cursor-pointer hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed";
  const btnSecondary =
    "px-3 py-1.5 text-sm font-medium rounded-md bg-surface-2 text-text-primary border border-border cursor-pointer hover:bg-surface-3 disabled:opacity-50 disabled:cursor-not-allowed";
</script>

<main class="max-w-[900px] mx-auto px-8 py-6">
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

  {#if error}
    <div class="mb-4">
      <AlertBanner variant="err" message={error} />
    </div>
  {/if}

  <h1 class="text-xl font-bold text-text-primary mb-6">Receiver</h1>

  <!-- Status + Profile two-column grid -->
  <div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-6">
    <!-- Status Card -->
    <Card title="Status">
      <section data-testid="status-section">
        {#if status}
          <dl
            class="grid gap-2 text-sm"
            style="grid-template-columns: auto 1fr;"
          >
            <dt class="text-text-muted">Connection</dt>
            <dd data-testid="connection-state">
              <StatusBadge
                label={connectionState}
                state={connectionBadgeState}
              />
            </dd>
            <dt class="text-text-muted">Local DB</dt>
            <dd>
              <StatusBadge
                label={status.local_ok ? "ok" : "error"}
                state={status.local_ok ? "ok" : "err"}
              />
            </dd>
            <dt class="text-text-muted">Streams</dt>
            <dd class="font-mono text-text-primary">{status.streams_count}</dd>
          </dl>
          <div class="flex gap-2 mt-3 pt-3 border-t border-border">
            <button
              class={btnPrimary}
              onclick={handleConnect}
              disabled={connectBusy || connectionState === "connected"}
            >
              Connect
            </button>
            <button
              class="px-3 py-1.5 text-sm font-medium rounded-md text-status-err border border-status-err-border bg-status-err-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
              onclick={handleDisconnect}
              disabled={connectBusy || connectionState === "disconnected"}
            >
              Disconnect
            </button>
          </div>
        {/if}
      </section>
    </Card>

    <!-- Config Card -->
    <Card title="Config">
      <section data-testid="config-section">
        <div class="grid gap-3">
          <label class="block text-xs font-medium text-text-muted">
            Server URL
            <input
              data-testid="server-url-input"
              class="{inputClass} mt-1"
              bind:value={editServerUrl}
              placeholder="wss://server:8080"
            />
          </label>
          <label class="block text-xs font-medium text-text-muted">
            Token
            <input
              data-testid="token-input"
              type="password"
              class="{inputClass} mt-1"
              bind:value={editToken}
              placeholder="auth token"
            />
          </label>
          <label class="block text-xs font-medium text-text-muted">
            Update Mode
            <select
              data-testid="update-mode-select"
              class="{inputClass} mt-1"
              bind:value={editUpdateMode}
            >
              <option value="check-and-download"
                >Automatic (check and download)</option
              >
              <option value="check-only"
                >Check Only (notify but don't download)</option
              >
              <option value="disabled">Disabled</option>
            </select>
          </label>
        </div>
        <div class="flex items-center gap-2 mt-3">
          <button
            data-testid="save-config-btn"
            class={btnPrimary}
            onclick={saveProfile}
            disabled={saving}
          >
            {saving ? "Saving..." : "Save"}
          </button>
          <button
            data-testid="check-update-btn"
            class={btnSecondary}
            onclick={handleCheckUpdate}
            disabled={checkingUpdate}
          >
            {checkingUpdate ? "Checking..." : "Check Now"}
          </button>
        </div>
        {#if checkMessage}
          <p class="text-xs mt-1 m-0 text-text-muted">{checkMessage}</p>
        {/if}
      </section>
    </Card>
  </div>

  <div class="mb-6">
    <Card title="Race & Mode Selection">
      <section data-testid="selection-section">
        <div class="grid gap-3">
          <label class="block text-xs font-medium text-text-muted">
            Mode
            <select
              data-testid="selection-mode-select"
              class="{inputClass} mt-1"
              bind:value={selectionMode}
              onchange={handleSelectionModeChange}
              disabled={selectionBusy}
            >
              <option value="manual">Manual</option>
              <option value="race">Race</option>
            </select>
          </label>

          {#if selectionMode === "race"}
            <label class="block text-xs font-medium text-text-muted">
              Race ID
              <select
                data-testid="race-id-select"
                class="{inputClass} mt-1"
                bind:value={raceIdDraft}
                onchange={handleRaceIdChange}
                disabled={selectionBusy}
              >
                <option value="">Select race...</option>
                {#each races as race}
                  <option value={race.race_id}>{race.name}</option>
                {/each}
              </select>
            </label>

            <label class="block text-xs font-medium text-text-muted">
              Epoch Scope
              <select
                data-testid="epoch-scope-select"
                class="{inputClass} mt-1"
                bind:value={epochScopeDraft}
                onchange={handleEpochScopeChange}
                disabled={selectionBusy}
              >
                <option value="current">Current and future</option>
                <option value="all">All</option>
              </select>
              <p class="text-xs text-text-muted mt-1 m-0">
                Current and future: replay the current epoch and continue
                receiving as the epoch advances. All: replay all epochs
                available for the race.
              </p>
            </label>
          {/if}

          <label class="block text-xs font-medium text-text-muted">
            Replay Policy
            <select
              data-testid="replay-policy-select"
              class="{inputClass} mt-1"
              bind:value={replayPolicyDraft}
              onchange={handleReplayPolicyChange}
              disabled={selectionBusy}
            >
              <option value="resume">Resume</option>
              <option value="live_only">Live only</option>
              <option value="targeted">Targeted replay</option>
            </select>
            <p class="text-xs text-text-muted mt-1 m-0">
              Resume: continue from the last acknowledged position. Live only:
              skip replay and receive new reads only. Targeted replay: replay
              full selected epochs per stream.
            </p>
          </label>

          {#if replayPolicyDraft === "targeted"}
            <div class="border border-border rounded-md p-3 bg-surface-0">
              {#if epochLoadError}
                <div class="mb-2">
                  <AlertBanner variant="err" message={epochLoadError} />
                </div>
              {/if}
              <div class="flex items-center justify-between mb-2">
                <p class="text-xs font-semibold text-text-primary m-0">
                  Replay Targets
                </p>
                <button
                  data-testid="add-targeted-row-btn"
                  class={btnSecondary}
                  onclick={addTargetedRow}
                  disabled={selectionBusy}
                >
                  Add Row
                </button>
              </div>

              <div class="grid gap-2">
                {#each targetedRows as row, index}
                  <div
                    class="grid gap-2 md:grid-cols-[2fr_2fr_auto] items-start"
                  >
                    <select
                      data-testid={"targeted-row-stream-" + index}
                      class={inputClass}
                      value={row.streamKey}
                      onchange={(event) =>
                        handleTargetedStreamChange(index, event)}
                      disabled={selectionBusy}
                    >
                      <option value="">Select stream...</option>
                      {#each streams?.streams ?? [] as stream}
                        <option
                          value={streamKey(
                            stream.forwarder_id,
                            stream.reader_ip,
                          )}
                        >
                          {stream.display_alias ??
                            `${stream.forwarder_id} / ${stream.reader_ip}`}
                        </option>
                      {/each}
                    </select>

                    <select
                      data-testid={"targeted-row-epoch-" + index}
                      class={inputClass}
                      value={row.streamEpoch}
                      onchange={(event) =>
                        handleTargetedEpochChange(index, event)}
                      disabled={selectionBusy || !row.streamKey}
                    >
                      <option value="">Select epoch...</option>
                      {#each targetedRowEpochOptions(row) as option}
                        <option value={String(option.stream_epoch)}>
                          {targetedEpochOptionLabel(option)}
                        </option>
                      {/each}
                    </select>

                    <button
                      data-testid={"remove-targeted-row-" + index}
                      class={btnSecondary}
                      onclick={() => removeTargetedRow(index)}
                      disabled={selectionBusy}
                    >
                      Remove
                    </button>

                    {#if targetedRowErrors[index]}
                      <p
                        data-testid={"targeted-row-error-" + index}
                        class="md:col-span-4 text-xs text-status-err m-0"
                      >
                        {#if targetedRowErrors[index].streamKey}
                          {targetedRowErrors[index].streamKey}
                        {/if}
                        {#if targetedRowErrors[index].streamEpoch}
                          {targetedRowErrors[index].streamKey ? " " : ""}
                          {targetedRowErrors[index].streamEpoch}
                        {/if}
                      </p>
                    {/if}
                  </div>
                {/each}
              </div>
            </div>
          {/if}
        </div>
        <div class="mt-3 pt-3 border-t border-border">
          <button
            data-testid="save-selection-btn"
            class={btnPrimary}
            onclick={() => void applySelection()}
            disabled={!isDirty || selectionBusy}
          >
            {selectionBusy ? "Saving..." : "Save"}
          </button>
        </div>
      </section>
    </Card>
  </div>

  <!-- Streams Section -->
  <div class="mb-6">
    <Card>
      {#snippet header()}
        <div class="flex items-center justify-between w-full">
          <h2 class="text-sm font-semibold text-text-primary">
            Available Streams
            {#if streams?.degraded}
              <span class="text-status-warn text-xs font-normal ml-1"
                >(degraded)</span
              >
            {/if}
          </h2>
          <span class="text-xs text-text-muted"
            >{subscribedCount} subscribed / {totalCount} available</span
          >
        </div>
      {/snippet}

      <section data-testid="streams-section" class="-mx-4 -mb-4">
        {#if streams?.upstream_error}
          <div class="px-4 py-2">
            <AlertBanner variant="warn" message={streams.upstream_error} />
          </div>
        {/if}
        {#if streams?.streams.length === 0}
          <p class="px-4 py-6 text-sm text-text-muted text-center m-0">
            No streams available.
          </p>
        {:else}
          <div class="divide-y divide-border">
            {#each streams?.streams ?? [] as stream}
              {@const key = streamKey(stream.forwarder_id, stream.reader_ip)}
              <div class="px-4 py-3 flex items-center gap-3">
                <div class="flex-1 min-w-0">
                  <div class="flex items-center gap-2">
                    <span
                      class="text-sm font-medium text-text-primary truncate"
                    >
                      {stream.display_alias ??
                        `${stream.forwarder_id} / ${stream.reader_ip}`}
                    </span>
                    {#if stream.online !== undefined}
                      <StatusBadge
                        label={stream.online ? "online" : "offline"}
                        state={stream.online ? "ok" : "err"}
                      />
                    {/if}
                  </div>
                  <p class="text-xs font-mono text-text-muted mt-0.5 m-0">
                    {stream.forwarder_id} / {stream.reader_ip}
                  </p>
                  {#if stream.stream_epoch !== undefined}
                    <p class="text-xs font-mono text-text-muted mt-0.5 m-0">
                      epoch: {stream.stream_epoch}{#if stream.current_epoch_name && stream.current_epoch_name.trim().length > 0}
                        {" "}({stream.current_epoch_name.trim()}){/if}
                    </p>
                  {/if}
                  {#if stream.subscribed && stream.reads_total !== undefined}
                    <p class="text-xs font-mono text-text-muted mt-0.5 m-0">
                      reads: {stream.reads_total} total{#if stream.reads_epoch !== undefined},
                        {stream.reads_epoch} epoch{/if}
                    </p>
                  {/if}
                </div>
                <div class="flex items-center gap-2 shrink-0">
                  {#if stream.subscribed}
                    <span class="text-xs font-mono text-text-secondary"
                      >port {stream.local_port ?? "auto"}</span
                    >
                    <button
                      data-testid="unsub-{key}"
                      class="px-2.5 py-1 text-xs font-medium rounded-md text-status-err border border-status-err-border bg-status-err-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
                      onclick={() =>
                        toggleSubscription(
                          stream.forwarder_id,
                          stream.reader_ip,
                          true,
                        )}
                      disabled={subscriptionsBusy}
                    >
                      {subscriptionsBusy && activeSubscriptionKey === key
                        ? "..."
                        : "Unsubscribe"}
                    </button>
                  {:else}
                    <input
                      data-testid="port-{key}"
                      type="number"
                      min="1"
                      max="65535"
                      placeholder="port"
                      aria-label="Port for {stream.display_alias ?? key}"
                      class="px-2 py-1 text-xs rounded font-mono bg-surface-0 border border-border text-text-primary w-20 focus:outline-none focus:ring-1 focus:ring-accent"
                      bind:value={portOverrides[key]}
                      disabled={subscriptionsBusy}
                    />
                    <button
                      data-testid="sub-{key}"
                      class="{btnPrimary} !px-2.5 !py-1 !text-xs"
                      onclick={() =>
                        toggleSubscription(
                          stream.forwarder_id,
                          stream.reader_ip,
                          false,
                        )}
                      disabled={subscriptionsBusy}
                    >
                      {subscriptionsBusy && activeSubscriptionKey === key
                        ? "..."
                        : "Subscribe"}
                    </button>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
        {/if}
      </section>
    </Card>
  </div>

  <!-- Logs -->
  <Card>
    <div class="-m-4">
      <LogViewer entries={logs?.entries ?? []} />
    </div>
  </Card>
</main>
