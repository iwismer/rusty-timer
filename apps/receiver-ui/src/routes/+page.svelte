<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import * as api from "$lib/api";
  import { buildUpdatedSubscriptions } from "$lib/subscriptions";
  import { initSSE, destroySSE } from "$lib/sse";
  import type {
    Profile,
    StatusResponse,
    StreamsResponse,
    LogsResponse,
  } from "$lib/api";

  let profile: Profile | null = null;
  let status: StatusResponse | null = null;
  let streams: StreamsResponse | null = null;
  let logs: LogsResponse | null = null;
  let error: string | null = null;

  // Edit state
  let editServerUrl = "";
  let editToken = "";
  let editLogLevel = "info";
  let saving = false;
  let connectBusy = false;
  let sseConnected = false;
  let portOverrides: Record<string, string | number | null> = {};
  let subscriptionsBusy = false;
  let activeSubscriptionKey: string | null = null;

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
        editLogLevel = p.log_level;
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
        log_level: editLogLevel,
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
    });
  });

  onDestroy(() => {
    destroySSE();
  });
</script>

<main>
  <h1>Rusty Timer Receiver</h1>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  <!-- Status -->
  <section data-testid="status-section">
    <h2>Status</h2>
    {#if status}
      <p data-testid="connection-state">
        Connection: {status.connection_state}
      </p>
      <p>Local OK: {status.local_ok}</p>
      <p>Streams: {status.streams_count}</p>
      <button
        on:click={handleConnect}
        disabled={connectBusy || status.connection_state === "connected"}
      >
        Connect
      </button>
      <button
        on:click={handleDisconnect}
        disabled={connectBusy || status.connection_state === "disconnected"}
      >
        Disconnect
      </button>
    {/if}
  </section>

  <!-- Profile -->
  <section data-testid="profile-section">
    <h2>Profile</h2>
    <label>
      Server URL
      <input
        data-testid="server-url-input"
        bind:value={editServerUrl}
        placeholder="wss://server:8080/ws/v1/receivers"
      />
    </label>
    <label>
      Token
      <input
        data-testid="token-input"
        type="password"
        bind:value={editToken}
        placeholder="auth token"
      />
    </label>
    <label>
      Log Level
      <select data-testid="log-level-select" bind:value={editLogLevel}>
        <option value="trace">trace</option>
        <option value="debug">debug</option>
        <option value="info">info</option>
        <option value="warn">warn</option>
        <option value="error">error</option>
      </select>
    </label>
    <button
      data-testid="save-profile-btn"
      on:click={saveProfile}
      disabled={saving}
    >
      {saving ? "Saving..." : "Save Profile"}
    </button>
  </section>

  <!-- Streams -->
  <section data-testid="streams-section">
    <h2>
      Streams {#if streams?.degraded}<span class="degraded">(degraded)</span
        >{/if}
    </h2>
    {#if streams?.upstream_error}
      <p class="warning">{streams.upstream_error}</p>
    {/if}
    {#if streams?.streams.length === 0}
      <p>No streams available.</p>
    {:else}
      <ul>
        {#each streams?.streams ?? [] as stream}
          {@const key = streamKey(stream.forwarder_id, stream.reader_ip)}
          <li>
            <span>
              {stream.display_alias ??
                `${stream.forwarder_id} / ${stream.reader_ip}`}
            </span>
            {#if stream.online !== undefined}
              <span class={stream.online ? "online" : "offline"}
                >{stream.online ? "(online)" : "(offline)"}</span
              >
            {/if}
            {#if stream.subscribed}
              <span>→ port {stream.local_port ?? "auto"}</span>
              <button
                data-testid="unsub-{key}"
                on:click={() =>
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
                class="port-input"
                data-testid="port-{key}"
                type="number"
                min="1"
                max="65535"
                placeholder="port"
                bind:value={portOverrides[key]}
                disabled={subscriptionsBusy}
              />
              <button
                data-testid="sub-{key}"
                on:click={() =>
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
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <!-- Logs -->
  <section data-testid="logs-section">
    <h2>Logs</h2>
    {#if logs?.entries.length === 0}
      <p>No log entries.</p>
    {:else}
      <ul class="logs">
        {#each logs?.entries ?? [] as entry}
          <li>{entry}</li>
        {/each}
      </ul>
    {/if}
  </section>
</main>

<style>
  main {
    max-width: 800px;
    margin: 0 auto;
    padding: 1rem;
    font-family: sans-serif;
  }
  section {
    margin-bottom: 2rem;
    border: 1px solid #ccc;
    padding: 1rem;
    border-radius: 4px;
  }
  h2 {
    margin-top: 0;
  }
  label {
    display: block;
    margin-bottom: 0.5rem;
  }
  input,
  select {
    display: block;
    width: 100%;
    margin-top: 0.25rem;
    padding: 0.25rem;
  }
  button {
    margin: 0.25rem;
    padding: 0.5rem 1rem;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .error {
    color: red;
  }
  .warning {
    color: orange;
  }
  .degraded {
    color: orange;
    font-size: 0.8em;
  }
  .online {
    color: green;
    font-size: 0.85em;
  }
  .offline {
    color: gray;
    font-size: 0.85em;
  }
  .logs {
    font-family: monospace;
    font-size: 0.85em;
    max-height: 300px;
    overflow-y: auto;
  }
  section ul:not(.logs) {
    list-style: none;
    padding: 0;
  }
  section ul:not(.logs) li {
    display: flex;
    align-items: center;
    gap: 0.5em;
    padding: 0.35em 0;
    border-bottom: 1px solid #eee;
  }
  section ul:not(.logs) li:last-child {
    border-bottom: none;
  }
  .port-input {
    width: 5em;
    display: inline-block;
  }
</style>
