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
  } from "$lib/api";
  type SupportedReplayPolicy = "resume" | "live_only";

  let profile = $state<Profile | null>(null);
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
  let replayPolicyDraft = $state<SupportedReplayPolicy>("resume");
  let selectionBusy = $state(false);
  let selectionApplyQueued = $state(false);

  function normalizeReplayPolicy(
    replayPolicy: ReplayPolicy,
  ): SupportedReplayPolicy {
    return replayPolicy === "live_only" ? "live_only" : "resume";
  }

  function streamKey(forwarder_id: string, reader_ip: string): string {
    return `${forwarder_id}/${reader_ip}`;
  }

  function applyStreamCountUpdates(updates: StreamCountUpdate[]) {
    if (!streams || updates.length === 0) {
      return;
    }

    const updatesByKey = new Map(
      updates.map((u) => [streamKey(u.forwarder_id, u.reader_ip), u]),
    );

    streams = {
      ...streams,
      streams: streams.streams.map((stream) => {
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
        if (nextSelection.selection.mode === "manual") {
          selectedStreams = nextSelection.selection.streams;
          raceIdDraft = "";
          epochScopeDraft = "current";
        } else {
          raceIdDraft = nextSelection.selection.race_id;
          epochScopeDraft = nextSelection.selection.epoch_scope;
          selectedStreams = [];
        }
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
    }
  }

  function selectionPayload(): api.ReceiverSetSelection {
    if (selectionMode === "manual") {
      return {
        selection: {
          mode: "manual",
          streams: selectedStreams,
        },
        replay_policy: replayPolicyDraft,
      };
    }

    return {
      selection: {
        mode: "race",
        race_id: raceIdDraft.trim(),
        epoch_scope: epochScopeDraft,
      },
      replay_policy: replayPolicyDraft,
    };
  }

  async function applySelection(): Promise<void> {
    selectionApplyQueued = true;
    if (selectionBusy) return;

    selectionBusy = true;
    error = null;
    try {
      while (selectionApplyQueued) {
        selectionApplyQueued = false;
        await api.putSelection(selectionPayload());
      }
    } catch (e) {
      error = String(e);
    } finally {
      selectionBusy = false;
    }
  }

  function handleSelectionModeChange(event: Event): void {
    const nextMode = (event.currentTarget as HTMLSelectElement).value as
      | "manual"
      | "race";
    selectionMode = nextMode;
    void applySelection();
  }

  function handleRaceIdBlur(): void {
    raceIdDraft = raceIdDraft.trim();
    if (selectionMode === "race") {
      void applySelection();
    }
  }

  function handleEpochScopeChange(event: Event): void {
    epochScopeDraft = (event.currentTarget as HTMLSelectElement)
      .value as EpochScope;
    void applySelection();
  }

  function handleReplayPolicyChange(event: Event): void {
    replayPolicyDraft = (event.currentTarget as HTMLSelectElement)
      .value as SupportedReplayPolicy;
    void applySelection();
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
        applyStreamCountUpdates(updates);
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
              class={btnSecondary}
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
    <Card title="Selection">
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
              <input
                data-testid="race-id-input"
                class="{inputClass} mt-1"
                bind:value={raceIdDraft}
                onblur={handleRaceIdBlur}
                list="race-id-options"
                placeholder="race id"
                disabled={selectionBusy}
              />
              <datalist id="race-id-options">
                {#each races as race}
                  <option value={race.race_id}>{race.name}</option>
                {/each}
              </datalist>
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
                <option value="current">Current</option>
                <option value="all">All</option>
              </select>
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
            </select>
          </label>
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
                  {#if stream.reads_total !== undefined}
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
