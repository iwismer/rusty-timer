<script lang="ts">
  import { onMount } from "svelte";
  import { Card } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";

  let streams = $state<api.StreamEntry[]>([]);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let inFlightKeys = $state<Set<string>>(new Set());
  let feedback = $state<{ message: string; ok: boolean } | null>(null);

  function streamKey(stream: api.StreamRef): string {
    return `${stream.forwarder_id}/${stream.reader_ip}`;
  }

  function streamLabel(stream: api.StreamEntry): string {
    return (
      stream.display_alias ?? `${stream.forwarder_id} / ${stream.reader_ip}`
    );
  }

  async function loadStreams() {
    loading = true;
    loadError = null;
    try {
      const response = await api.getStreams();
      streams = response.streams;
    } catch {
      streams = [];
      loadError = "Failed to load streams.";
    } finally {
      loading = false;
    }
  }

  async function handleReset(stream: api.StreamEntry) {
    const key = streamKey(stream);
    inFlightKeys = new Set(inFlightKeys).add(key);
    feedback = null;
    try {
      await api.resetStreamCursor({
        forwarder_id: stream.forwarder_id,
        reader_ip: stream.reader_ip,
      });
      feedback = {
        message: `Cursor reset for ${streamLabel(stream)}.`,
        ok: true,
      };
    } catch {
      feedback = {
        message: `Failed to reset cursor for ${streamLabel(stream)}.`,
        ok: false,
      };
    } finally {
      const next = new Set(inFlightKeys);
      next.delete(key);
      inFlightKeys = next;
    }
  }

  onMount(loadStreams);
</script>

<svelte:head>
  <title>Receiver Admin · Rusty Timer</title>
</svelte:head>

<main class="max-w-[960px] mx-auto px-6 py-6">
  <div class="flex items-center justify-between mb-6">
    <h1 class="text-xl font-bold text-text-primary m-0">Receiver Admin</h1>
  </div>

  {#if feedback}
    <p
      class="text-sm mb-4 m-0 {feedback.ok
        ? 'text-status-ok'
        : 'text-status-err'}"
      data-testid="admin-feedback"
    >
      {feedback.message}
    </p>
  {/if}

  <Card title="Cursor Reset" borderStatus="warn">
    <p class="text-sm text-text-muted m-0 mb-4">
      Reset resume cursors per stream. The selected stream will replay from the
      beginning on next connect.
    </p>

    {#if loading}
      <p class="text-sm text-text-muted m-0">Loading streams...</p>
    {:else if loadError}
      <p class="text-sm text-status-err m-0">{loadError}</p>
    {:else if streams.length === 0}
      <p class="text-sm text-text-muted m-0">No streams available.</p>
    {:else}
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-border text-left text-text-muted">
            <th class="py-2 pr-4 font-medium">Stream</th>
            <th class="py-2 pr-4 font-medium">Forwarder</th>
            <th class="py-2 pr-4 font-medium">Reader</th>
            <th class="py-2 font-medium"></th>
          </tr>
        </thead>
        <tbody>
          {#each streams as stream (streamKey(stream))}
            {@const key = streamKey(stream)}
            <tr class="border-b border-border/50">
              <td class="py-2 pr-4">
                {#if stream.display_alias}
                  <span class="text-text-primary font-medium"
                    >{stream.display_alias}</span
                  >
                  <span class="block text-xs text-text-muted"
                    >{stream.forwarder_id} / {stream.reader_ip}</span
                  >
                {:else}
                  <span class="text-text-primary"
                    >{stream.forwarder_id} / {stream.reader_ip}</span
                  >
                {/if}
              </td>
              <td class="py-2 pr-4 text-text-secondary"
                >{stream.forwarder_id}</td
              >
              <td class="py-2 pr-4 text-text-secondary">{stream.reader_ip}</td>
              <td class="py-2 text-right">
                <button
                  onclick={() => handleReset(stream)}
                  disabled={inFlightKeys.has(key)}
                  class="px-2.5 py-1 text-xs font-medium rounded-md text-status-warn border border-status-warn-border bg-status-warn-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
                  aria-label={"Reset cursor for " + streamLabel(stream)}
                >
                  {inFlightKeys.has(key) ? "Resetting..." : "Reset Cursor"}
                </button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </Card>
</main>
