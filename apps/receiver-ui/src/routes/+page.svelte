<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import * as api from "$lib/api";
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
    RaceEntry,
    StreamCountUpdate,
    StatusResponse,
    StreamsResponse,
    LogsResponse,
    ReceiverMode,
  } from "$lib/api";

  let status = $state<StatusResponse | null>(null);
  let streams = $state<StreamsResponse | null>(null);
  let logs = $state<LogsResponse | null>(null);
  let error = $state<string | null>(null);

  // Edit state
  let editServerUrl = $state("");
  let editToken = $state("");
  let editUpdateMode = $state("check-and-download");
  let checkingUpdate = $state(false);
  let checkMessage = $state<string | null>(null);
  let saving = $state(false);
  let connectBusy = $state(false);
  let updateVersion = $state<string | null>(null);
  let updateStatus = $state<"available" | "downloaded" | null>(null);
  let updateBusy = $state(false);

  let races = $state<RaceEntry[]>([]);
  let modeDraft = $state<ReceiverMode["mode"]>("live");
  let selectedLiveStreamKeys = $state<string[]>([]);
  let raceIdDraft = $state("");
  let earliestEpochInputs = $state<Record<string, string>>({});
  let targetedEpochInputs = $state<Record<string, string>>({});
  let modeBusy = $state(false);
  let modeApplyQueued = $state(false);
  let savedModePayload = $state<string | null>(null);
  let modeHydrationVersion = 0;
  let streamActionBusy = $state(false);
  let streamRefreshVersion = 0;

  let loadAllInFlight = false;
  let loadAllQueued = false;

  function streamKey(forwarder_id: string, reader_ip: string): string {
    return `${forwarder_id}/${reader_ip}`;
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

  function parseNonNegativeInt(raw: unknown): number | null {
    if (typeof raw === "number") {
      if (!Number.isSafeInteger(raw) || raw < 0) {
        return null;
      }
      return raw;
    }
    if (typeof raw !== "string") {
      return null;
    }
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

  function hydrateMode(mode: ReceiverMode): void {
    modeDraft = mode.mode;
    if (mode.mode === "live") {
      selectedLiveStreamKeys = mode.streams.map((s) =>
        streamKey(s.forwarder_id, s.reader_ip),
      );
      earliestEpochInputs = Object.fromEntries(
        mode.earliest_epochs.map((row) => [
          streamKey(row.forwarder_id, row.reader_ip),
          String(row.earliest_epoch),
        ]),
      );
      raceIdDraft = "";
      targetedEpochInputs = {};
      return;
    }

    if (mode.mode === "race") {
      raceIdDraft = mode.race_id;
      targetedEpochInputs = {};
      return;
    }

    targetedEpochInputs = Object.fromEntries(
      mode.targets.map((target) => [
        streamKey(target.forwarder_id, target.reader_ip),
        String(target.stream_epoch),
      ]),
    );
  }

  function applyHydratedMode(mode: ReceiverMode): void {
    hydrateMode(mode);
    savedModePayload = JSON.stringify(mode);
    modeHydrationVersion += 1;
  }

  function modePayload(): ReceiverMode {
    if (modeDraft === "race") {
      return {
        mode: "race",
        race_id: raceIdDraft.trim(),
      };
    }

    if (modeDraft === "targeted_replay") {
      const targets = Object.entries(targetedEpochInputs)
        .map(([key, value]) => {
          const stream = parseStreamKey(key);
          const stream_epoch = parseNonNegativeInt(value);
          if (!stream || stream_epoch === null) {
            return null;
          }
          return {
            forwarder_id: stream.forwarder_id,
            reader_ip: stream.reader_ip,
            stream_epoch,
          };
        })
        .filter((target): target is api.ReplayTarget => target !== null);

      return {
        mode: "targeted_replay",
        targets,
      };
    }

    const streams = selectedLiveStreamKeys
      .map(parseStreamKey)
      .filter((stream): stream is api.StreamRef => stream !== null);

    const earliest_epochs = Object.entries(earliestEpochInputs)
      .map(([key, value]) => {
        const stream = parseStreamKey(key);
        const earliest_epoch = parseNonNegativeInt(value);
        if (!stream || earliest_epoch === null) {
          return null;
        }
        return {
          forwarder_id: stream.forwarder_id,
          reader_ip: stream.reader_ip,
          earliest_epoch,
        };
      })
      .filter(
        (
          row,
        ): row is {
          forwarder_id: string;
          reader_ip: string;
          earliest_epoch: number;
        } => row !== null,
      );

    return {
      mode: "live",
      streams,
      earliest_epochs,
    };
  }

  let modeDirty = $derived(
    savedModePayload !== null && JSON.stringify(modePayload()) !== savedModePayload,
  );

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

  async function loadAll() {
    if (loadAllInFlight) {
      loadAllQueued = true;
      return;
    }

    loadAllInFlight = true;
    try {
      const modeVersionAtLoadStart = modeHydrationVersion;
      const [nextStatus, nextStreams, nextLogs, nextMode, nextRaces] =
        await Promise.all([
          api.getStatus(),
          api.getStreams(),
          api.getLogs(),
          api.getMode().catch(() => null),
          api.getRaces().catch(() => ({ races: [] })),
        ]);
      status = nextStatus;
      streams = nextStreams;
      logs = nextLogs;
      races = nextRaces.races;

      if (
        nextMode &&
        !modeDirty &&
        modeHydrationVersion === modeVersionAtLoadStart
      ) {
        applyHydratedMode(nextMode);
      }

      const p = await api.getProfile().catch(() => null);
      if (p) {
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

  function toggleLiveStreamSelection(forwarder_id: string, reader_ip: string): void {
    const key = streamKey(forwarder_id, reader_ip);
    if (selectedLiveStreamKeys.includes(key)) {
      selectedLiveStreamKeys = selectedLiveStreamKeys.filter((k) => k !== key);
      return;
    }
    selectedLiveStreamKeys = [...selectedLiveStreamKeys, key];
  }

  async function applyMode(): Promise<void> {
    modeApplyQueued = true;
    if (modeBusy) return;

    modeBusy = true;
    error = null;

    while (modeApplyQueued) {
      modeApplyQueued = false;
      const payload = modePayload();
      if (payload.mode === "race" && payload.race_id.length === 0) {
        error = "Select a race before applying Race mode.";
        continue;
      }

      try {
        await api.putMode(payload);
        savedModePayload = JSON.stringify(payload);
        error = null;
      } catch (e) {
        error = String(e);
        if (!modeApplyQueued) {
          break;
        }
      }
    }

    modeBusy = false;
  }

  async function setEarliestEpoch(forwarder_id: string, reader_ip: string): Promise<void> {
    if (modeDraft === "race") {
      return;
    }

    const key = streamKey(forwarder_id, reader_ip);
    const parsed = parseNonNegativeInt(earliestEpochInputs[key] ?? "");
    if (parsed === null) {
      error = "Earliest epoch must be a non-negative integer.";
      return;
    }

    try {
      error = null;
      await api.putEarliestEpoch({ forwarder_id, reader_ip, earliest_epoch: parsed });
      earliestEpochInputs = { ...earliestEpochInputs, [key]: String(parsed) };
    } catch (e) {
      error = String(e);
    }
  }

  async function pauseOrResumeStream(stream: api.StreamEntry): Promise<void> {
    if (streamActionBusy) {
      return;
    }

    streamActionBusy = true;
    const refreshVersion = ++streamRefreshVersion;
    try {
      error = null;
      if (stream.paused) {
        await api.resumeStream({
          forwarder_id: stream.forwarder_id,
          reader_ip: stream.reader_ip,
        });
      } else {
        await api.pauseStream({
          forwarder_id: stream.forwarder_id,
          reader_ip: stream.reader_ip,
        });
      }
      const latestStreams = await api.getStreams();
      if (refreshVersion === streamRefreshVersion) {
        streams = latestStreams;
      }
    } catch (e) {
      error = String(e);
    } finally {
      streamActionBusy = false;
    }
  }

  async function pauseOrResumeAll(action: "pause" | "resume"): Promise<void> {
    if (streamActionBusy) {
      return;
    }

    streamActionBusy = true;
    const refreshVersion = ++streamRefreshVersion;
    try {
      error = null;
      if (action === "pause") {
        await api.pauseAll();
      } else {
        await api.resumeAll();
      }
      const latestStreams = await api.getStreams();
      if (refreshVersion === streamRefreshVersion) {
        streams = latestStreams;
      }
    } catch (e) {
      error = String(e);
    } finally {
      streamActionBusy = false;
    }
  }

  async function replayStream(forwarder_id: string, reader_ip: string): Promise<void> {
    const key = streamKey(forwarder_id, reader_ip);
    const parsed = parseNonNegativeInt(targetedEpochInputs[key] ?? "");
    if (parsed === null) {
      error = "Target epoch must be a non-negative integer.";
      return;
    }

    try {
      error = null;
      const payload: ReceiverMode = {
        mode: "targeted_replay",
        targets: [{ forwarder_id, reader_ip, stream_epoch: parsed }],
      };
      await api.putMode(payload);
      savedModePayload = JSON.stringify(payload);
    } catch (e) {
      error = String(e);
    }
  }

  async function replayAll(): Promise<void> {
    const targets = Object.entries(targetedEpochInputs)
      .map(([key, value]) => {
        const stream = parseStreamKey(key);
        const stream_epoch = parseNonNegativeInt(value);
        if (!stream || stream_epoch === null) {
          return null;
        }
        return {
          forwarder_id: stream.forwarder_id,
          reader_ip: stream.reader_ip,
          stream_epoch,
        };
      })
      .filter((row): row is api.ReplayTarget => row !== null);

    if (targets.length === 0) {
      error = "Enter at least one valid target epoch before replaying all.";
      return;
    }

    try {
      error = null;
      const payload: ReceiverMode = {
        mode: "targeted_replay",
        targets,
      };
      await api.putMode(payload);
      savedModePayload = JSON.stringify(payload);
    } catch (e) {
      error = String(e);
    }
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
        checkMessage = null;
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
        if (!connected) {
          return;
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
      onStreamCountsUpdated: (updates) => {
        const needsResync = applyStreamCountUpdates(updates);
        if (needsResync) {
          void loadAll();
        }
      },
      onModeChanged: (mode) => {
        applyHydratedMode(mode);
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

  <div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-6">
    <Card title="Status">
      <section data-testid="status-section">
        {#if status}
          <dl class="grid gap-2 text-sm" style="grid-template-columns: auto 1fr;">
            <dt class="text-text-muted">Connection</dt>
            <dd data-testid="connection-state">
              <StatusBadge label={connectionState} state={connectionBadgeState} />
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
    <Card title="Receiver Mode">
      <section data-testid="mode-section">
        <div class="grid gap-3">
          <label class="block text-xs font-medium text-text-muted">
            Mode
            <select
              data-testid="mode-select"
              class="{inputClass} mt-1"
              bind:value={modeDraft}
              disabled={modeBusy}
            >
              <option value="live">Live</option>
              <option value="race">Race</option>
              <option value="targeted_replay">Targeted Replay</option>
            </select>
          </label>

          {#if modeDraft === "race"}
            <label class="block text-xs font-medium text-text-muted">
              Race
              <select
                data-testid="race-id-select"
                class="{inputClass} mt-1"
                bind:value={raceIdDraft}
                disabled={modeBusy}
              >
                <option value="">Select race...</option>
                {#each races as race}
                  <option value={race.race_id}>{race.name}</option>
                {/each}
              </select>
            </label>
          {/if}

          {#if modeDraft === "live"}
            <p class="text-xs text-text-muted m-0">
              Live mode uses stream checkboxes in the table below and supports
              earliest-epoch overrides.
            </p>
          {:else if modeDraft === "race"}
            <p class="text-xs text-text-muted m-0">
              Race mode follows race stream resolution from the server; earliest
              epoch controls are shown but disabled.
            </p>
          {:else}
            <p class="text-xs text-text-muted m-0">
              Targeted Replay uses per-stream epoch controls in the table.
            </p>
          {/if}
        </div>

        <div class="mt-3 pt-3 border-t border-border">
          <button
            data-testid="save-mode-btn"
            class={btnPrimary}
            onclick={() => void applyMode()}
            disabled={!modeDirty || modeBusy}
          >
            {modeBusy ? "Applying..." : "Apply Mode"}
          </button>
        </div>
      </section>
    </Card>
  </div>

  <div class="mb-6">
    <Card>
      {#snippet header()}
        <div class="flex items-center justify-between w-full gap-2">
          <h2 class="text-sm font-semibold text-text-primary">
            Available Streams
            {#if streams?.degraded}
              <span class="text-status-warn text-xs font-normal ml-1"
                >(degraded)</span
              >
            {/if}
          </h2>
          <div class="flex items-center gap-2">
            <span class="text-xs text-text-muted"
              >{subscribedCount} subscribed / {totalCount} available</span
            >
            {#if modeDraft === "live" || modeDraft === "race"}
              <button
                data-testid="pause-all-btn"
                class={btnSecondary}
                onclick={() => void pauseOrResumeAll("pause")}
                disabled={streamActionBusy}
              >
                Pause All
              </button>
              <button
                data-testid="resume-all-btn"
                class={btnSecondary}
                onclick={() => void pauseOrResumeAll("resume")}
                disabled={streamActionBusy}
              >
                Resume All
              </button>
            {:else}
              <button
                data-testid="replay-all-btn"
                class={btnSecondary}
                onclick={() => void replayAll()}
              >
                Replay All
              </button>
            {/if}
          </div>
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
                    <span class="text-sm font-medium text-text-primary truncate">
                      {stream.display_alias ??
                        `${stream.forwarder_id} / ${stream.reader_ip}`}
                    </span>
                    {#if stream.online !== undefined}
                      <StatusBadge
                        label={stream.online ? "online" : "offline"}
                        state={stream.online ? "ok" : "err"}
                      />
                    {/if}
                    <StatusBadge
                      label={stream.paused ? "paused" : "active"}
                      state={stream.paused ? "warn" : "ok"}
                    />
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
                  {#if modeDraft === "targeted_replay"}
                    <input
                      data-testid="targeted-epoch-{key}"
                      type="number"
                      min="0"
                      class="px-2 py-1 text-xs rounded font-mono bg-surface-0 border border-border text-text-primary w-24 focus:outline-none focus:ring-1 focus:ring-accent"
                      bind:value={targetedEpochInputs[key]}
                      placeholder="epoch"
                    />
                    <button
                      data-testid="replay-stream-{key}"
                      class="{btnPrimary} !px-2.5 !py-1 !text-xs"
                      onclick={() =>
                        replayStream(stream.forwarder_id, stream.reader_ip)}
                    >
                      Replay
                    </button>
                  {:else}
                    {#if modeDraft === "live"}
                      <label class="text-xs text-text-muted inline-flex items-center gap-1">
                        <input
                          data-testid="live-stream-toggle-{key}"
                          type="checkbox"
                          checked={selectedLiveStreamKeys.includes(key)}
                          onchange={() =>
                            toggleLiveStreamSelection(
                              stream.forwarder_id,
                              stream.reader_ip,
                            )}
                        />
                        Include
                      </label>
                    {/if}

                    <input
                      data-testid="earliest-epoch-{key}"
                      type="number"
                      min="0"
                      class="px-2 py-1 text-xs rounded font-mono bg-surface-0 border border-border text-text-primary w-24 focus:outline-none focus:ring-1 focus:ring-accent"
                      bind:value={earliestEpochInputs[key]}
                      placeholder="earliest"
                      disabled={modeDraft === "race"}
                    />
                    <button
                      data-testid="apply-earliest-{key}"
                      class={btnSecondary}
                      onclick={() =>
                        setEarliestEpoch(stream.forwarder_id, stream.reader_ip)}
                      disabled={modeDraft === "race"}
                    >
                      Set Earliest
                    </button>
                    <button
                      data-testid="pause-resume-{key}"
                      class={btnSecondary}
                      onclick={() => pauseOrResumeStream(stream)}
                      disabled={streamActionBusy}
                    >
                      {stream.paused ? "Resume" : "Pause"}
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

  <Card>
    <div class="-m-4">
      <LogViewer entries={logs?.entries ?? []} />
    </div>
  </Card>
</main>
