<script lang="ts">
  import { onMount } from "svelte";
  import { Card, ConfirmDialog } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import { createLatestRequestGate } from "$lib/latestRequestGate";
  import type {
    StreamEntry,
    TokenEntry,
    CreateTokenResponse,
    CursorEntry,
    EpochInfo,
  } from "$lib/api";

  // ----- Shared state -----
  let streams: StreamEntry[] = $state([]);
  let tokens: TokenEntry[] = $state([]);
  let cursors: CursorEntry[] = $state([]);
  let busy = $state(false);
  let races: api.RaceEntry[] = $state([]);
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
  let selectedStreamId = $state("");
  let selectedEpochValue = $state("");
  let epochs: EpochInfo[] = $state([]);
  let epochsLoading = $state(false);
  let epochsError = $state(false);
  const epochRequestGate = createLatestRequestGate();

  onMount(() => {
    loadStreams();
    loadTokens();
    loadCursors();
    loadRaces();
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

  async function loadCursors() {
    try {
      const resp = await api.getCursors();
      cursors = resp.cursors;
    } catch {
      cursors = [];
    }
  }

  async function loadRaces() {
    try {
      const resp = await api.getRaces();
      races = resp.races;
    } catch {
      races = [];
    }
  }

  async function loadEpochs(streamId: string) {
    if (!streamId) {
      epochRequestGate.invalidate();
      epochs = [];
      epochsError = false;
      epochsLoading = false;
      return;
    }
    const token = epochRequestGate.next();
    epochsLoading = true;
    epochsError = false;
    try {
      const data = await api.getStreamEpochs(streamId);
      if (!epochRequestGate.isLatest(token) || streamId !== selectedStreamId)
        return;
      epochs = data;
    } catch {
      if (!epochRequestGate.isLatest(token) || streamId !== selectedStreamId)
        return;
      epochs = [];
      epochsError = true;
    } finally {
      if (epochRequestGate.isLatest(token)) {
        epochsLoading = false;
      }
    }
  }

  function handleStreamChange() {
    selectedEpochValue = "";
    loadEpochs(selectedStreamId);
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
      await loadCursors();
      await loadRaces();
      if (selectedStreamId) await loadEpochs(selectedStreamId);
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
    if (!selectedStreamId) {
      showConfirm(
        "Clear all events?",
        "This will permanently delete all events across every stream.",
        "Clear All Events",
        () => api.deleteAllEvents(),
      );
    } else if (!selectedEpochValue) {
      const s = streams.find((s) => s.stream_id === selectedStreamId);
      showConfirm(
        "Clear stream events?",
        `This will permanently delete all events for "${s?.display_alias || s?.reader_ip || selectedStreamId}".`,
        "Clear Stream Events",
        () => api.deleteStreamEvents(selectedStreamId),
      );
    } else {
      const epochNum = Number(selectedEpochValue);
      const s = streams.find((s) => s.stream_id === selectedStreamId);
      const epochInfo = epochs.find((e) => e.epoch === epochNum);
      const countStr = epochInfo ? ` (${epochInfo.event_count} events)` : "";
      showConfirm(
        "Clear epoch events?",
        `This will permanently delete all events for "${s?.display_alias || s?.reader_ip || selectedStreamId}", epoch ${epochNum}${countStr}.`,
        `Clear Epoch ${epochNum} Events`,
        () => api.deleteEpochEvents(selectedStreamId, epochNum),
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

  function confirmDeleteAllTokens() {
    showConfirm(
      "Delete All Tokens",
      "This will permanently delete ALL device tokens (active and revoked). Connected devices will lose access. This cannot be undone.",
      "Delete All",
      () => api.deleteAllTokens(),
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
      await loadTokens();
    } catch (e) {
      feedback = { message: String(e), ok: false };
    } finally {
      creating = false;
    }
  }

  function dismissReveal() {
    revealedToken = null;
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

  function confirmDeleteReceiverCursors(receiverId: string) {
    showConfirm(
      "Clear Receiver Cursors",
      `This will clear all cursor positions for receiver "${receiverId}". It will re-sync from the beginning on its next connection.`,
      "Clear Cursors",
      () => api.deleteReceiverCursors(receiverId),
    );
  }

  function confirmDeleteReceiverStreamCursor(
    receiverId: string,
    streamId: string,
  ) {
    const s = streams.find((s) => s.stream_id === streamId);
    const streamName = s?.display_alias || s?.reader_ip || streamId;
    showConfirm(
      "Clear Cursor",
      `This will clear the cursor for receiver "${receiverId}" on stream "${streamName}".`,
      "Clear Cursor",
      () => api.deleteReceiverStreamCursor(receiverId, streamId),
    );
  }

  function confirmDeleteAllRaces() {
    showConfirm(
      "Delete All Races",
      "This will permanently delete ALL races and all associated data (participants, chips, and forwarder associations). This cannot be undone.",
      "Delete All",
      () => api.deleteAllRaces(),
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
      <p class="text-sm text-text-muted m-0 mb-4">
        Manage active and offline streams. Deleting a stream permanently removes
        it along with all associated events, metrics, and receiver cursors.
      </p>
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
      <p class="text-sm text-text-muted m-0 mb-4">
        Delete stored timing events. You can clear events globally, for a
        specific stream, or for a specific epoch within a stream.
      </p>
      <div class="flex flex-col gap-4">
        <div class="flex flex-col gap-1">
          <label
            for="event-stream-select"
            class="text-xs font-medium text-text-muted"
          >
            Stream
          </label>
          <select
            id="event-stream-select"
            bind:value={selectedStreamId}
            onchange={handleStreamChange}
            class="w-full max-w-sm px-3 py-2 text-sm rounded-md border border-border bg-surface-0 text-text-primary"
          >
            <option value="">All Streams</option>
            {#each streams as s (s.stream_id)}
              <option value={s.stream_id}
                >{s.display_alias || s.reader_ip}</option
              >
            {/each}
          </select>
        </div>

        {#if selectedStreamId}
          <div class="flex flex-col gap-1">
            <label
              for="event-epoch-select"
              class="text-xs font-medium text-text-muted"
            >
              Epoch
            </label>
            {#if epochsLoading}
              <p class="text-xs text-text-muted m-0">Loading epochs...</p>
            {:else if epochsError}
              <p class="text-xs text-status-err m-0">Failed to load epochs.</p>
            {:else}
              <select
                id="event-epoch-select"
                bind:value={selectedEpochValue}
                class="w-full max-w-sm px-3 py-2 text-sm rounded-md border border-border bg-surface-0 text-text-primary"
              >
                <option value="">All Epochs</option>
                {#each epochs as ep (ep.epoch)}
                  <option value={String(ep.epoch)}>
                    Epoch {ep.epoch}{ep.is_current ? " (current)" : ""} — {ep.event_count}
                    event{ep.event_count !== 1 ? "s" : ""}{ep.last_event_at
                      ? ` (${new Date(ep.last_event_at).toLocaleDateString()})`
                      : ""}
                  </option>
                {/each}
              </select>
            {/if}
          </div>
        {/if}

        <div>
          <button
            onclick={confirmClearEvents}
            disabled={epochsLoading}
            class="px-3 py-1.5 text-sm font-medium rounded-md bg-status-err-bg text-status-err border border-status-err-border cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {#if !selectedStreamId}
              Clear All Events
            {:else if !selectedEpochValue}
              Clear Stream Events
            {:else}
              Clear Epoch {selectedEpochValue} Events
            {/if}
          </button>
        </div>
      </div>
    </Card>
  </div>

  <!-- Device Tokens Section -->
  <div class="mb-6">
    <Card title="Device Tokens" borderStatus="err">
      <p class="text-sm text-text-muted m-0 mb-4">
        Create and manage authentication tokens for forwarders and receivers.
        Revoking a token prevents the device from connecting.
      </p>
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
      {#if tokens.length > 0}
        <div class="mt-4 pt-4 border-t border-border">
          <button
            onclick={confirmDeleteAllTokens}
            class="px-3 py-1.5 text-sm font-medium rounded-md bg-status-err text-white border-none cursor-pointer hover:opacity-80"
          >
            Delete All Tokens
          </button>
        </div>
      {/if}
    </Card>
  </div>

  <!-- Receiver Cursors Section -->
  <div class="mb-6">
    <Card title="Receiver Cursors" borderStatus="err">
      <p class="text-sm text-text-muted m-0 mb-4">
        Manage receiver sync positions. Clearing a cursor causes the receiver to
        re-sync from the beginning on its next connection.
      </p>
      {#if cursors.length === 0}
        <p class="text-sm text-text-muted m-0">No receiver cursors.</p>
      {:else}
        <table class="w-full text-sm">
          <thead>
            <tr class="border-b border-border text-left text-text-muted">
              <th class="py-2 pr-4 font-medium">Receiver</th>
              <th class="py-2 pr-4 font-medium">Stream</th>
              <th class="py-2 pr-4 font-medium">Epoch</th>
              <th class="py-2 pr-4 font-medium">Last Seq</th>
              <th class="py-2 pr-4 font-medium">Updated</th>
              <th class="py-2 font-medium"></th>
            </tr>
          </thead>
          <tbody>
            {#each cursors as c, i (c.receiver_id + c.stream_id)}
              <tr class="border-b border-border/50">
                <td class="py-2 pr-4 text-text-primary">
                  {c.receiver_id}
                </td>
                <td class="py-2 pr-4 text-text-secondary">
                  {streams.find((s) => s.stream_id === c.stream_id)
                    ?.display_alias ||
                    streams.find((s) => s.stream_id === c.stream_id)
                      ?.reader_ip ||
                    c.stream_id}
                </td>
                <td class="py-2 pr-4 text-text-secondary">{c.stream_epoch}</td>
                <td class="py-2 pr-4 text-text-secondary">{c.last_seq}</td>
                <td class="py-2 pr-4 text-text-muted text-xs">
                  {new Date(c.updated_at).toLocaleString()}
                </td>
                <td class="py-2 text-right whitespace-nowrap">
                  <button
                    onclick={() =>
                      confirmDeleteReceiverStreamCursor(
                        c.receiver_id,
                        c.stream_id,
                      )}
                    class="px-2 py-1 text-xs font-medium rounded bg-status-err-bg text-status-err border border-status-err-border cursor-pointer hover:opacity-80"
                  >
                    Delete
                  </button>
                </td>
              </tr>
              <!-- Show "Delete All for Receiver" after last cursor row for this receiver -->
              {#if i === cursors.length - 1 || cursors[i + 1].receiver_id !== c.receiver_id}
                {#if cursors.filter((x) => x.receiver_id === c.receiver_id).length > 1}
                  <tr class="border-b border-border/30">
                    <td colspan="6" class="py-2 text-right">
                      <button
                        onclick={() =>
                          confirmDeleteReceiverCursors(c.receiver_id)}
                        class="px-2 py-1 text-xs font-medium rounded bg-status-err-bg text-status-err border border-status-err-border cursor-pointer hover:opacity-80"
                      >
                        Delete All for {c.receiver_id}
                      </button>
                    </td>
                  </tr>
                {/if}
              {/if}
            {/each}
          </tbody>
        </table>
      {/if}
      <div class="mt-4 pt-4 border-t border-border">
        <button
          onclick={confirmClearCursors}
          class="px-3 py-1.5 text-sm font-medium rounded-md bg-status-err text-white border-none cursor-pointer hover:opacity-80"
        >
          Clear All Cursors
        </button>
      </div>
    </Card>
  </div>

  <!-- Races Section -->
  <div class="mb-6">
    <Card title="Races" borderStatus="err">
      <p class="text-sm text-text-muted m-0 mb-4">
        Delete all races and associated data. This removes all races,
        participants, chip mappings, and forwarder-race associations.
      </p>
      <p class="text-sm text-text-secondary m-0 mb-4">
        {races.length}
        {races.length === 1 ? "race" : "races"}
      </p>
      <div>
        <button
          onclick={confirmDeleteAllRaces}
          class="px-3 py-1.5 text-sm font-medium rounded-md bg-status-err text-white border-none cursor-pointer hover:opacity-80"
        >
          Delete All Races
        </button>
      </div>
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
