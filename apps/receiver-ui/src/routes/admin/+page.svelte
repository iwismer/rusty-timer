<script lang="ts">
  import { onMount } from "svelte";
  import { Card } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";

  let streams = $state<api.StreamEntry[]>([]);
  let subscriptions = $state<api.SubscriptionItem[]>([]);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let inFlightKeys = $state<Set<string>>(new Set());
  let inFlightAction = $state<string | null>(null);
  let feedback = $state<{ message: string; ok: boolean } | null>(null);
  let confirmingFactoryReset = $state(false);

  // Port editing state: keyed by "forwarder_id/reader_ip"
  let portEdits = $state<Map<string, string>>(new Map());

  function streamKey(stream: {
    forwarder_id: string;
    reader_ip: string;
  }): string {
    return `${stream.forwarder_id}/${stream.reader_ip}`;
  }

  function streamLabel(stream: api.StreamEntry): string {
    return (
      stream.display_alias ?? `${stream.forwarder_id} / ${stream.reader_ip}`
    );
  }

  function setFeedback(message: string, ok: boolean) {
    feedback = { message, ok };
  }

  async function loadAll() {
    loading = true;
    loadError = null;
    try {
      const [streamsResp, subsResp] = await Promise.all([
        api.getStreams(),
        api.getSubscriptions(),
      ]);
      streams = streamsResp.streams;
      subscriptions = subsResp.subscriptions;
    } catch {
      streams = [];
      subscriptions = [];
      loadError = "Failed to load data.";
    } finally {
      loading = false;
    }
  }

  // --- Cursor reset (per-stream) ---
  async function handleResetCursor(stream: api.StreamEntry) {
    const key = streamKey(stream);
    inFlightKeys = new Set(inFlightKeys).add(key);
    feedback = null;
    try {
      await api.resetStreamCursor({
        forwarder_id: stream.forwarder_id,
        reader_ip: stream.reader_ip,
      });
      setFeedback(`Cursor reset for ${streamLabel(stream)}.`, true);
    } catch {
      setFeedback(`Failed to reset cursor for ${streamLabel(stream)}.`, false);
    } finally {
      const next = new Set(inFlightKeys);
      next.delete(key);
      inFlightKeys = next;
    }
  }

  // --- Bulk actions ---
  async function handleBulkAction(
    action: () => Promise<{ deleted: number } | void>,
    label: string,
    actionId: string,
  ) {
    inFlightAction = actionId;
    feedback = null;
    try {
      const result = await action();
      if (result && typeof result === "object" && "deleted" in result) {
        setFeedback(`${label}: ${result.deleted} item(s) removed.`, true);
      } else {
        setFeedback(`${label}: done.`, true);
      }
      await loadAll();
    } catch {
      setFeedback(`${label}: failed.`, false);
    } finally {
      inFlightAction = null;
    }
  }

  // --- Earliest epoch reset (per-stream) ---
  async function handleResetEpoch(stream: api.StreamEntry) {
    const key = `epoch-${streamKey(stream)}`;
    inFlightKeys = new Set(inFlightKeys).add(key);
    feedback = null;
    try {
      await api.resetEarliestEpoch({
        forwarder_id: stream.forwarder_id,
        reader_ip: stream.reader_ip,
      });
      setFeedback(
        `Earliest-epoch override reset for ${streamLabel(stream)}.`,
        true,
      );
    } catch {
      setFeedback(
        `Failed to reset earliest-epoch for ${streamLabel(stream)}.`,
        false,
      );
    } finally {
      const next = new Set(inFlightKeys);
      next.delete(key);
      inFlightKeys = next;
    }
  }

  // --- Port override ---
  function getPortDisplayValue(sub: api.SubscriptionItem): string {
    const key = streamKey(sub);
    if (portEdits.has(key)) return portEdits.get(key)!;
    return sub.local_port_override?.toString() ?? "";
  }

  function handlePortInput(sub: api.SubscriptionItem, value: string) {
    const next = new Map(portEdits);
    next.set(streamKey(sub), value);
    portEdits = next;
  }

  function isPortDirty(sub: api.SubscriptionItem): boolean {
    const key = streamKey(sub);
    if (!portEdits.has(key)) return false;
    const editVal = portEdits.get(key)!;
    const currentVal = sub.local_port_override?.toString() ?? "";
    return editVal !== currentVal;
  }

  async function handleSavePort(sub: api.SubscriptionItem) {
    const key = streamKey(sub);
    const raw = portEdits.get(key) ?? "";
    const trimmed = raw.trim();
    let portValue: number | null = null;
    if (trimmed !== "") {
      if (!/^\d+$/.test(trimmed)) {
        setFeedback("Port must be 1-65535 or empty to clear.", false);
        return;
      }
      const parsed = Number(trimmed);
      if (!Number.isInteger(parsed) || parsed < 1 || parsed > 65535) {
        setFeedback("Port must be 1-65535 or empty to clear.", false);
        return;
      }
      portValue = parsed;
    }

    const actionKey = `port-${key}`;
    inFlightKeys = new Set(inFlightKeys).add(actionKey);
    feedback = null;
    try {
      await api.updateLocalPort(
        { forwarder_id: sub.forwarder_id, reader_ip: sub.reader_ip },
        portValue,
      );
      setFeedback(
        portValue
          ? `Port override set to ${portValue} for ${sub.forwarder_id} / ${sub.reader_ip}.`
          : `Port override cleared for ${sub.forwarder_id} / ${sub.reader_ip}.`,
        true,
      );
      const next = new Map(portEdits);
      next.delete(key);
      portEdits = next;
      await loadAll();
    } catch {
      setFeedback(
        `Failed to update port for ${sub.forwarder_id} / ${sub.reader_ip}.`,
        false,
      );
    } finally {
      const next = new Set(inFlightKeys);
      next.delete(actionKey);
      inFlightKeys = next;
    }
  }

  // --- Factory reset ---
  async function handleFactoryReset() {
    confirmingFactoryReset = false;
    await handleBulkAction(
      () => api.factoryReset(),
      "Factory reset",
      "factory-reset",
    );
  }

  onMount(loadAll);
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

  {#if loading}
    <p class="text-sm text-text-muted">Loading...</p>
  {:else if loadError}
    <p class="text-sm text-status-err">{loadError}</p>
  {:else}
    <div class="space-y-6">
      <!-- Cursor Reset -->
      <Card title="Cursor Reset" borderStatus="warn">
        <p class="text-sm text-text-muted m-0 mb-4">
          Reset resume cursors per stream. The selected stream will replay from
          the beginning on next connect.
        </p>

        {#if streams.length === 0}
          <p class="text-sm text-text-muted m-0">No streams available.</p>
        {:else}
          <table class="w-full text-sm">
            <thead>
              <tr class="border-b border-border text-left text-text-muted">
                <th class="py-2 pr-4 font-medium">Stream</th>
                <th class="py-2 pr-4 font-medium">Forwarder</th>
                <th class="py-2 pr-4 font-medium">Reader</th>
                <th class="py-2 pr-4 font-medium">Epoch</th>
                <th class="py-2 pr-4 font-medium">Seq</th>
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
                  <td class="py-2 pr-4 text-text-secondary"
                    >{stream.reader_ip}</td
                  >
                  <td class="py-2 pr-4 text-text-secondary tabular-nums"
                    >{stream.cursor_epoch ?? "\u2014"}</td
                  >
                  <td class="py-2 pr-4 text-text-secondary tabular-nums"
                    >{stream.cursor_seq ?? "\u2014"}</td
                  >
                  <td class="py-2 text-right">
                    <button
                      onclick={() => handleResetCursor(stream)}
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
          <div class="mt-4 flex justify-end">
            <button
              onclick={() =>
                handleBulkAction(
                  () => api.resetAllCursors(),
                  "Reset all cursors",
                  "reset-all-cursors",
                )}
              disabled={inFlightAction === "reset-all-cursors"}
              class="px-3 py-1.5 text-xs font-medium rounded-md text-status-warn border border-status-warn-border bg-status-warn-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {inFlightAction === "reset-all-cursors"
                ? "Resetting..."
                : "Reset All Cursors"}
            </button>
          </div>
        {/if}
      </Card>

      <!-- Earliest-Epoch Overrides -->
      <Card title="Earliest-Epoch Overrides" borderStatus="warn">
        <p class="text-sm text-text-muted m-0 mb-4">
          Clear earliest-epoch overrides per stream or all at once. Streams will
          revert to receiving all available epochs.
        </p>

        {#if streams.length === 0}
          <p class="text-sm text-text-muted m-0">No streams available.</p>
        {:else}
          <table class="w-full text-sm">
            <thead>
              <tr class="border-b border-border text-left text-text-muted">
                <th class="py-2 pr-4 font-medium">Stream</th>
                <th class="py-2 font-medium"></th>
              </tr>
            </thead>
            <tbody>
              {#each streams as stream (streamKey(stream))}
                {@const key = `epoch-${streamKey(stream)}`}
                <tr class="border-b border-border/50">
                  <td class="py-2 pr-4">
                    <span class="text-text-primary">{streamLabel(stream)}</span>
                  </td>
                  <td class="py-2 text-right">
                    <button
                      onclick={() => handleResetEpoch(stream)}
                      disabled={inFlightKeys.has(key)}
                      class="px-2.5 py-1 text-xs font-medium rounded-md text-status-warn border border-status-warn-border bg-status-warn-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
                      aria-label={"Reset epoch for " + streamLabel(stream)}
                    >
                      {inFlightKeys.has(key) ? "Resetting..." : "Reset Epoch"}
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
          <div class="mt-4 flex justify-end">
            <button
              onclick={() =>
                handleBulkAction(
                  () => api.resetAllEarliestEpochs(),
                  "Reset all earliest-epoch overrides",
                  "reset-all-epochs",
                )}
              disabled={inFlightAction === "reset-all-epochs"}
              class="px-3 py-1.5 text-xs font-medium rounded-md text-status-warn border border-status-warn-border bg-status-warn-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {inFlightAction === "reset-all-epochs"
                ? "Resetting..."
                : "Reset All Epoch Overrides"}
            </button>
          </div>
        {/if}
      </Card>

      <!-- Local Port Overrides -->
      <Card title="Local Port Overrides">
        <p class="text-sm text-text-muted m-0 mb-4">
          Set or clear the local forwarding port per subscription. Leave empty
          to use the default port.
        </p>

        {#if subscriptions.length === 0}
          <p class="text-sm text-text-muted m-0">No subscriptions.</p>
        {:else}
          <table class="w-full text-sm">
            <thead>
              <tr class="border-b border-border text-left text-text-muted">
                <th class="py-2 pr-4 font-medium">Forwarder</th>
                <th class="py-2 pr-4 font-medium">Reader</th>
                <th class="py-2 pr-4 font-medium">Port Override</th>
                <th class="py-2 font-medium"></th>
              </tr>
            </thead>
            <tbody>
              {#each subscriptions as sub (streamKey(sub))}
                {@const portKey = `port-${streamKey(sub)}`}
                <tr class="border-b border-border/50">
                  <td class="py-2 pr-4 text-text-secondary"
                    >{sub.forwarder_id}</td
                  >
                  <td class="py-2 pr-4 text-text-secondary">{sub.reader_ip}</td>
                  <td class="py-2 pr-4">
                    <input
                      type="text"
                      inputmode="numeric"
                      placeholder="default"
                      value={getPortDisplayValue(sub)}
                      oninput={(e) =>
                        handlePortInput(
                          sub,
                          (e.target as HTMLInputElement).value,
                        )}
                      class="w-24 px-2 py-1 text-sm rounded border border-border bg-bg-primary text-text-primary"
                    />
                  </td>
                  <td class="py-2 text-right">
                    <button
                      onclick={() => handleSavePort(sub)}
                      disabled={!isPortDirty(sub) || inFlightKeys.has(portKey)}
                      class="px-2.5 py-1 text-xs font-medium rounded-md text-text-primary border border-border bg-bg-secondary cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
                    >
                      {inFlightKeys.has(portKey) ? "Saving..." : "Save"}
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </Card>

      <!-- Purge Subscriptions -->
      <Card title="Purge Subscriptions" borderStatus="warn">
        <p class="text-sm text-text-muted m-0 mb-4">
          Remove all stream subscriptions. The receiver will have no streams
          until new ones are added.
        </p>
        <button
          onclick={() =>
            handleBulkAction(
              () => api.purgeSubscriptions(),
              "Purge subscriptions",
              "purge-subs",
            )}
          disabled={inFlightAction === "purge-subs" ||
            subscriptions.length === 0}
          class="px-3 py-1.5 text-xs font-medium rounded-md text-status-warn border border-status-warn-border bg-status-warn-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {inFlightAction === "purge-subs"
            ? "Purging..."
            : "Purge All Subscriptions"}
        </button>
      </Card>

      <!-- Reset Profile -->
      <Card title="Reset Profile" borderStatus="warn">
        <p class="text-sm text-text-muted m-0 mb-4">
          Clear server URL, token, and receiver ID back to defaults. The
          receiver will need to be reconfigured before connecting.
        </p>
        <button
          onclick={() =>
            handleBulkAction(
              () => api.resetProfile(),
              "Reset profile",
              "reset-profile",
            )}
          disabled={inFlightAction === "reset-profile"}
          class="px-3 py-1.5 text-xs font-medium rounded-md text-status-warn border border-status-warn-border bg-status-warn-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {inFlightAction === "reset-profile"
            ? "Resetting..."
            : "Reset Profile to Defaults"}
        </button>
      </Card>

      <!-- Factory Reset -->
      <Card title="Factory Reset" borderStatus="err">
        <p class="text-sm text-text-muted m-0 mb-4">
          Clear <strong>all</strong> local data: profile, subscriptions, cursors,
          and epoch overrides. The receiver will disconnect and return to a fresh
          state. This cannot be undone.
        </p>
        {#if confirmingFactoryReset}
          <div class="flex items-center gap-3">
            <span class="text-sm text-status-err font-medium"
              >Are you sure?</span
            >
            <button
              onclick={handleFactoryReset}
              disabled={inFlightAction === "factory-reset"}
              class="px-3 py-1.5 text-xs font-medium rounded-md text-white bg-status-err border border-status-err cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {inFlightAction === "factory-reset"
                ? "Resetting..."
                : "Yes, Factory Reset"}
            </button>
            <button
              onclick={() => (confirmingFactoryReset = false)}
              class="px-3 py-1.5 text-xs font-medium rounded-md text-text-secondary border border-border bg-bg-secondary cursor-pointer hover:opacity-80"
            >
              Cancel
            </button>
          </div>
        {:else}
          <button
            onclick={() => (confirmingFactoryReset = true)}
            class="px-3 py-1.5 text-xs font-medium rounded-md text-status-err border border-status-err bg-transparent cursor-pointer hover:opacity-80"
          >
            Factory Reset...
          </button>
        {/if}
      </Card>
    </div>
  {/if}
</main>
