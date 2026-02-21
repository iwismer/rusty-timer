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
    Profile,
    StatusResponse,
    StreamsResponse,
    LogsResponse,
  } from "$lib/api";

  let profile = $state<Profile | null>(null);
  let status = $state<StatusResponse | null>(null);
  let streams = $state<StreamsResponse | null>(null);
  let logs = $state<LogsResponse | null>(null);
  let error = $state<string | null>(null);

  // Edit state
  let editServerUrl = $state("");
  let editToken = $state("");
  let saving = $state(false);
  let connectBusy = $state(false);
  let sseConnected = $state(false);
  let updateVersion = $state<string | null>(null);
  let updateBusy = $state(false);
  let portOverrides = $state<Record<string, string | number | null>>({});
  let subscriptionsBusy = $state(false);
  let activeSubscriptionKey = $state<string | null>(null);

  function streamKey(forwarder_id: string, reader_ip: string): string {
    return `${forwarder_id}/${reader_ip}`;
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
      [status, streams, logs] = await Promise.all([
        api.getStatus(),
        api.getStreams(),
        api.getLogs(),
      ]);
      const p = await api.getProfile().catch(() => null);
      if (p) {
        profile = p;
        editServerUrl = p.server_url;
        editToken = p.token;
      }
      const updateStatus = await api.getUpdateStatus().catch(() => null);
      if (updateStatus?.status === "downloaded" && updateStatus.version) {
        updateVersion = updateStatus.version;
      } else if (updateStatus?.status === "up_to_date") {
        updateVersion = null;
      }
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
      });
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
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
      onUpdateAvailable: (version, _currentVersion) => {
        updateVersion = version;
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
  {#if updateVersion}
    <div class="mb-4">
      <UpdateBanner
        version={updateVersion}
        busy={updateBusy}
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
            <dt class="text-text-muted">Local OK</dt>
            <dd class="text-text-primary">{status.local_ok}</dd>
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

    <!-- Profile Card -->
    <Card title="Profile">
      <section data-testid="profile-section">
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
        </div>
        <button
          data-testid="save-profile-btn"
          class="{btnPrimary} mt-3"
          onclick={saveProfile}
          disabled={saving}
        >
          {saving ? "Saving..." : "Save Profile"}
        </button>
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
