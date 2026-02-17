<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "$lib/api";
  import type { StreamEntry } from "$lib/api";

  let streams: StreamEntry[] = [];
  let loading = true;
  let error: string | null = null;

  // Per-stream rename state (keyed by stream_id)
  let renameValues: Record<string, string> = {};
  let renameBusy: Record<string, boolean> = {};
  let renameError: Record<string, string | null> = {};

  async function loadStreams() {
    loading = true;
    error = null;
    try {
      const resp = await api.getStreams();
      streams = resp.streams;
      // Initialise rename inputs with current alias (or empty string)
      for (const s of streams) {
        renameValues[s.stream_id] = s.display_alias ?? "";
      }
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  async function handleRename(streamId: string) {
    renameBusy[streamId] = true;
    renameError[streamId] = null;
    try {
      const updated = await api.renameStream(streamId, renameValues[streamId]);
      // Update the stream entry in place
      streams = streams.map((s) => (s.stream_id === streamId ? updated : s));
    } catch (e) {
      renameError[streamId] = String(e);
    } finally {
      renameBusy[streamId] = false;
    }
  }

  onMount(loadStreams);
</script>

<main>
  <h1 data-testid="streams-heading">Dashboard – Streams</h1>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  {#if loading}
    <p>Loading…</p>
  {:else}
    <ul data-testid="stream-list">
      {#each streams as stream (stream.stream_id)}
        <li data-testid="stream-item">
          <div class="stream-header">
            <!-- Display alias or fallback to forwarder_id / reader_ip -->
            <a
              data-testid="stream-detail-link"
              href="/streams/{stream.stream_id}"
            >
              {#if stream.display_alias}
                <strong>{stream.display_alias}</strong>
              {:else}
                <strong>{stream.forwarder_id}</strong> / {stream.reader_ip}
              {/if}
            </a>

            {#if stream.online}
              <span data-testid="stream-online-badge" class="badge online"
                >online</span
              >
            {:else}
              <span data-testid="stream-offline-badge" class="badge offline"
                >offline</span
              >
            {/if}
          </div>

          <div class="stream-meta">
            <span>forwarder: {stream.forwarder_id}</span>
            <span>reader: {stream.reader_ip}</span>
            <span>epoch: {stream.stream_epoch}</span>
          </div>

          <!-- Rename form -->
          <div class="rename-row">
            <input
              data-testid="rename-input"
              type="text"
              bind:value={renameValues[stream.stream_id]}
              placeholder="Display alias"
              aria-label="Rename stream {stream.stream_id}"
            />
            <button
              data-testid="rename-btn"
              on:click={() => handleRename(stream.stream_id)}
              disabled={renameBusy[stream.stream_id]}
            >
              {renameBusy[stream.stream_id] ? "Saving…" : "Rename"}
            </button>
          </div>

          {#if renameError[stream.stream_id]}
            <p class="error">{renameError[stream.stream_id]}</p>
          {/if}
        </li>
      {/each}

      {#if streams.length === 0}
        <li>No streams found.</li>
      {/if}
    </ul>
  {/if}
</main>

<style>
  main {
    max-width: 900px;
    margin: 0 auto;
    padding: 1rem;
    font-family: sans-serif;
  }
  ul {
    list-style: none;
    padding: 0;
  }
  li {
    border: 1px solid #ccc;
    padding: 0.75rem 1rem;
    margin-bottom: 0.75rem;
    border-radius: 4px;
  }
  .stream-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.4rem;
  }
  .stream-meta {
    font-size: 0.8em;
    color: #666;
    display: flex;
    gap: 1rem;
    margin-bottom: 0.4rem;
  }
  .rename-row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin-top: 0.4rem;
  }
  .rename-row input {
    flex: 1;
    padding: 0.25rem 0.5rem;
  }
  .badge {
    font-size: 0.75em;
    padding: 0.15rem 0.5rem;
    border-radius: 3px;
    font-weight: bold;
  }
  .online {
    background: #d4edda;
    color: #155724;
  }
  .offline {
    background: #f8d7da;
    color: #721c24;
  }
  a {
    text-decoration: none;
    color: #0070f3;
  }
  a:hover {
    text-decoration: underline;
  }
  button {
    padding: 0.25rem 0.75rem;
    cursor: pointer;
  }
  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .error {
    color: red;
    margin: 0.25rem 0;
    font-size: 0.85em;
  }
</style>
