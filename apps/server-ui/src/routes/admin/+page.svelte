<script lang="ts">
  import { onMount } from "svelte";
  import { Card, ConfirmDialog } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import type { StreamEntry, TokenEntry, CreateTokenResponse } from "$lib/api";

  // ----- Shared state -----
  let streams: StreamEntry[] = $state([]);
  let tokens: TokenEntry[] = $state([]);
  let busy = $state(false);
  let feedback: { message: string; ok: boolean } | null = $state(null);

  // ----- Create token state -----
  let newDeviceId = $state("");
  let newDeviceType: "forwarder" | "receiver" = $state("forwarder");
  let newTokenInput = $state("");
  let creating = $state(false);

  // ----- Token reveal state -----
  let revealedToken: CreateTokenResponse | null = $state(null);
  let copied = $state(false);

  // ----- Confirm dialog state -----
  let confirmOpen = $state(false);
  let confirmTitle = $state("");
  let confirmMessage = $state("");
  let confirmLabel = $state("Confirm");
  let confirmAction: (() => Promise<void>) | null = $state(null);

  // ----- Event scope state -----
  let eventScope: "all" | "stream" | "epoch" = $state("all");
  let selectedStreamId = $state("");
  let selectedEpoch = $state(1);
  let epochValid = $derived(
    Number.isInteger(selectedEpoch) && selectedEpoch >= 1,
  );

  onMount(() => {
    loadStreams();
    loadTokens();
  });

  async function loadStreams() {
    try {
      const resp = await api.getStreams();
      streams = resp.streams;
    } catch {
      streams = [];
    }
  }

  async function loadTokens() {
    try {
      const resp = await api.getTokens();
      tokens = resp.tokens;
    } catch {
      tokens = [];
    }
  }

  function showConfirm(
    title: string,
    message: string,
    label: string,
    action: () => Promise<void>,
  ) {
    confirmTitle = title;
    confirmMessage = message;
    confirmLabel = label;
    confirmAction = action;
    confirmOpen = true;
    feedback = null;
  }

  async function handleConfirm() {
    if (!confirmAction) return;
    busy = true;
    feedback = null;
    try {
      await confirmAction();
      confirmOpen = false;
      feedback = { message: "Done.", ok: true };
      await loadStreams();
      await loadTokens();
    } catch (e) {
      feedback = { message: String(e), ok: false };
    } finally {
      busy = false;
    }
  }

  function handleCancel() {
    if (busy) return;
    confirmOpen = false;
  }

  // ----- Stream actions -----
  function confirmDeleteStream(s: StreamEntry) {
    showConfirm(
      "Delete Stream",
      `This will permanently delete stream "${s.display_alias || s.reader_ip}" and all its events, metrics, and receiver cursors.`,
      "Delete",
      () => api.deleteStream(s.stream_id),
    );
  }

  function confirmDeleteAllStreams() {
    showConfirm(
      "Delete All Streams",
      "This will permanently delete ALL streams and ALL associated data (events, metrics, cursors). This cannot be undone.",
      "Delete All",
      () => api.deleteAllStreams(),
    );
  }

  // ----- Event actions -----
  function confirmClearEvents() {
    if (eventScope === "all") {
      showConfirm(
        "Clear All Events",
        "This will permanently delete ALL events across all streams.",
        "Clear All Events",
        () => api.deleteAllEvents(),
      );
    } else if (eventScope === "stream" && selectedStreamId) {
      const s = streams.find((s) => s.stream_id === selectedStreamId);
      showConfirm(
        "Clear Stream Events",
        `This will delete all events for stream "${s?.display_alias || s?.reader_ip || selectedStreamId}".`,
        "Clear Events",
        () => api.deleteStreamEvents(selectedStreamId),
      );
    } else if (eventScope === "epoch" && selectedStreamId && epochValid) {
      const s = streams.find((s) => s.stream_id === selectedStreamId);
      showConfirm(
        "Clear Epoch Events",
        `This will delete events for epoch ${selectedEpoch} of stream "${s?.display_alias || s?.reader_ip || selectedStreamId}".`,
        "Clear Events",
        () => api.deleteEpochEvents(selectedStreamId, selectedEpoch),
      );
    }
  }

  // ----- Token actions -----
  function confirmRevokeToken(t: TokenEntry) {
    showConfirm(
      "Revoke Token",
      `This will revoke the token for device "${t.device_id}" (${t.device_type}). The device will no longer be able to connect.`,
      "Revoke",
      () => api.revokeToken(t.token_id),
    );
  }

  // ----- Create token actions -----
  async function handleCreateToken() {
    creating = true;
    feedback = null;
    try {
      const result = await api.createToken({
        device_id: newDeviceId,
        device_type: newDeviceType,
        token: newTokenInput || undefined,
      });
      revealedToken = result;
      copied = false;
      newDeviceId = "";
      newDeviceType = "forwarder";
      newTokenInput = "";
    } catch (e) {
      feedback = { message: String(e), ok: false };
    } finally {
      creating = false;
    }
  }

  async function dismissReveal() {
    revealedToken = null;
    await loadTokens();
  }

  async function copyToken() {
    if (!revealedToken) return;
    await navigator.clipboard.writeText(revealedToken.token);
    copied = true;
  }

  function downloadToken() {
    if (!revealedToken) return;
    const blob = new Blob([revealedToken.token], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `${revealedToken.device_id}-token.txt`;
    a.click();
    URL.revokeObjectURL(url);
  }

  // ----- Cursor actions -----
  function confirmClearCursors() {
    showConfirm(
      "Clear All Receiver Cursors",
      "This will clear all receiver cursor positions. All receivers will re-sync from the beginning on their next connection.",
      "Clear Cursors",
      () => api.deleteAllCursors(),
    );
  }
</script>

<svelte:head>
  <title>Admin · Rusty Timer</title>
</svelte:head>

<main class="max-w-[1100px] mx-auto px-6 py-6">
  <div class="flex items-center justify-between mb-6">
    <h1 class="text-xl font-bold text-text-primary m-0">Admin</h1>
  </div>

  {#if feedback}
    <p
      class="text-sm mb-4 m-0 {feedback.ok
        ? 'text-status-ok'
        : 'text-status-err'}"
    >
      {feedback.message}
    </p>
  {/if}

  <!-- Streams Section -->
  <div class="mb-6">
    <Card title="Streams" borderStatus="err">
      {#if streams.length === 0}
        <p class="text-sm text-text-muted m-0">No streams.</p>
      {:else}
        <table class="w-full text-sm">
          <thead>
            <tr class="border-b border-border text-left text-text-muted">
              <th class="py-2 pr-4 font-medium">Stream</th>
              <th class="py-2 pr-4 font-medium">Forwarder</th>
              <th class="py-2 pr-4 font-medium">Status</th>
              <th class="py-2 font-medium"></th>
            </tr>
          </thead>
          <tbody>
            {#each streams as s (s.stream_id)}
              <tr class="border-b border-border/50">
                <td class="py-2 pr-4 text-text-primary">
                  {s.display_alias || s.reader_ip}
                </td>
                <td class="py-2 pr-4 text-text-secondary">
                  {s.forwarder_display_name || s.forwarder_id}
                </td>
                <td class="py-2 pr-4">
                  <span
                    class="text-xs {s.online
                      ? 'text-status-ok'
                      : 'text-text-muted'}"
                  >
                    {s.online ? "Online" : "Offline"}
                  </span>
                </td>
                <td class="py-2 text-right">
                  <button
                    onclick={() => confirmDeleteStream(s)}
                    class="px-2 py-1 text-xs font-medium rounded bg-status-err-bg text-status-err border border-status-err-border cursor-pointer hover:opacity-80"
                  >
                    Delete
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
        <div class="mt-4 pt-4 border-t border-border">
          <button
            onclick={confirmDeleteAllStreams}
            class="px-3 py-1.5 text-sm font-medium rounded-md bg-status-err text-white border-none cursor-pointer hover:opacity-80"
          >
            Delete All Streams
          </button>
        </div>
      {/if}
    </Card>
  </div>

  <!-- Events Section -->
  <div class="mb-6">
    <Card title="Events" borderStatus="err">
      <div class="flex flex-col gap-4">
        <fieldset class="flex gap-4 border-none p-0 m-0">
          <label
            class="flex items-center gap-1.5 text-sm text-text-secondary cursor-pointer"
          >
            <input
              type="radio"
              bind:group={eventScope}
              value="all"
              class="accent-accent"
            />
            All Events
          </label>
          <label
            class="flex items-center gap-1.5 text-sm text-text-secondary cursor-pointer"
          >
            <input
              type="radio"
              bind:group={eventScope}
              value="stream"
              class="accent-accent"
            />
            By Stream
          </label>
          <label
            class="flex items-center gap-1.5 text-sm text-text-secondary cursor-pointer"
          >
            <input
              type="radio"
              bind:group={eventScope}
              value="epoch"
              class="accent-accent"
            />
            By Stream + Epoch
          </label>
        </fieldset>

        {#if eventScope === "stream" || eventScope === "epoch"}
          <select
            bind:value={selectedStreamId}
            class="w-full max-w-sm px-3 py-2 text-sm rounded-md border border-border bg-surface-0 text-text-primary"
          >
            <option value="">Select a stream...</option>
            {#each streams as s (s.stream_id)}
              <option value={s.stream_id}
                >{s.display_alias || s.reader_ip}</option
              >
            {/each}
          </select>
        {/if}

        {#if eventScope === "epoch"}
          <input
            type="number"
            bind:value={selectedEpoch}
            min="1"
            class="w-full max-w-[8rem] px-3 py-2 text-sm rounded-md border border-border bg-surface-0 text-text-primary"
            placeholder="Epoch"
          />
          {#if !epochValid}
            <p class="text-xs text-status-err m-0">
              Epoch must be 1 or greater.
            </p>
          {/if}
        {/if}

        <div>
          <button
            onclick={confirmClearEvents}
            disabled={((eventScope === "stream" || eventScope === "epoch") &&
              !selectedStreamId) ||
              (eventScope === "epoch" && !epochValid)}
            class="px-3 py-1.5 text-sm font-medium rounded-md bg-status-err-bg text-status-err border border-status-err-border cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            Clear Events
          </button>
        </div>
      </div>
    </Card>
  </div>

  <!-- Device Tokens Section -->
  <div class="mb-6">
    <Card title="Device Tokens" borderStatus="err">
      <!-- Token Reveal Banner -->
      {#if revealedToken}
        <div
          class="mb-4 p-4 rounded-md border border-status-warn-border bg-status-warn-bg"
        >
          <p class="text-sm font-medium text-status-warn m-0 mb-2">
            Save this token now — it cannot be recovered.
          </p>
          <div class="flex gap-2 items-center mb-2">
            <input
              type="text"
              readonly
              value={revealedToken.token}
              class="flex-1 px-3 py-2 text-sm rounded-md border border-border bg-surface-0 text-text-primary font-mono"
            />
            <button
              onclick={copyToken}
              class="px-3 py-2 text-sm font-medium rounded-md bg-surface-1 text-text-primary border border-border cursor-pointer hover:opacity-80"
            >
              {copied ? "Copied!" : "Copy"}
            </button>
            <button
              onclick={downloadToken}
              class="px-3 py-2 text-sm font-medium rounded-md bg-surface-1 text-text-primary border border-border cursor-pointer hover:opacity-80"
            >
              Download
            </button>
          </div>
          <button
            onclick={dismissReveal}
            class="px-3 py-1.5 text-xs font-medium rounded-md bg-transparent text-text-muted border border-border cursor-pointer hover:opacity-80"
          >
            Dismiss
          </button>
        </div>
      {/if}

      <!-- Create Token Form -->
      <div class="mb-4 pb-4 border-b border-border">
        <h3 class="text-sm font-medium text-text-secondary m-0 mb-3">
          Create Token
        </h3>
        <div class="flex flex-col gap-3">
          <input
            type="text"
            bind:value={newDeviceId}
            placeholder="Device ID (required)"
            class="w-full max-w-sm px-3 py-2 text-sm rounded-md border border-border bg-surface-0 text-text-primary"
          />
          <fieldset class="flex gap-4 border-none p-0 m-0">
            <label
              class="flex items-center gap-1.5 text-sm text-text-secondary cursor-pointer"
            >
              <input
                type="radio"
                bind:group={newDeviceType}
                value="forwarder"
                class="accent-accent"
              />
              Forwarder
            </label>
            <label
              class="flex items-center gap-1.5 text-sm text-text-secondary cursor-pointer"
            >
              <input
                type="radio"
                bind:group={newDeviceType}
                value="receiver"
                class="accent-accent"
              />
              Receiver
            </label>
          </fieldset>
          <input
            type="text"
            bind:value={newTokenInput}
            placeholder="Leave blank to auto-generate"
            class="w-full max-w-sm px-3 py-2 text-sm rounded-md border border-border bg-surface-0 text-text-primary"
          />
          <div>
            <button
              onclick={handleCreateToken}
              disabled={!newDeviceId.trim() || creating}
              class="px-3 py-1.5 text-sm font-medium rounded-md bg-accent text-white border-none cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {creating ? "Creating..." : "Create Token"}
            </button>
          </div>
        </div>
      </div>

      <!-- Token List -->
      {#if tokens.length === 0}
        <p class="text-sm text-text-muted m-0">No tokens.</p>
      {:else}
        <table class="w-full text-sm">
          <thead>
            <tr class="border-b border-border text-left text-text-muted">
              <th class="py-2 pr-4 font-medium">Device</th>
              <th class="py-2 pr-4 font-medium">Type</th>
              <th class="py-2 pr-4 font-medium">Created</th>
              <th class="py-2 pr-4 font-medium">Status</th>
              <th class="py-2 font-medium"></th>
            </tr>
          </thead>
          <tbody>
            {#each tokens as t (t.token_id)}
              <tr
                class="border-b border-border/50 {t.revoked
                  ? 'opacity-50'
                  : ''}"
              >
                <td class="py-2 pr-4 text-text-primary">{t.device_id}</td>
                <td class="py-2 pr-4 text-text-secondary">{t.device_type}</td>
                <td class="py-2 pr-4 text-text-muted text-xs">
                  {new Date(t.created_at).toLocaleDateString()}
                </td>
                <td class="py-2 pr-4">
                  <span
                    class="text-xs {t.revoked
                      ? 'text-status-err'
                      : 'text-status-ok'}"
                  >
                    {t.revoked ? "Revoked" : "Active"}
                  </span>
                </td>
                <td class="py-2 text-right">
                  {#if !t.revoked}
                    <button
                      onclick={() => confirmRevokeToken(t)}
                      class="px-2 py-1 text-xs font-medium rounded bg-status-err-bg text-status-err border border-status-err-border cursor-pointer hover:opacity-80"
                    >
                      Revoke
                    </button>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </Card>
  </div>

  <!-- Receiver Cursors Section -->
  <div class="mb-6">
    <Card title="Receiver Cursors" borderStatus="err">
      <p class="text-sm text-text-secondary m-0 mb-4">
        Clear all receiver cursor positions. This forces receivers to re-sync
        from the beginning on their next connection.
      </p>
      <button
        onclick={confirmClearCursors}
        class="px-3 py-1.5 text-sm font-medium rounded-md bg-status-err-bg text-status-err border border-status-err-border cursor-pointer hover:opacity-80"
      >
        Clear All Cursors
      </button>
    </Card>
  </div>
</main>

<ConfirmDialog
  open={confirmOpen}
  title={confirmTitle}
  message={confirmMessage}
  {confirmLabel}
  {busy}
  onConfirm={handleConfirm}
  onCancel={handleCancel}
/>
