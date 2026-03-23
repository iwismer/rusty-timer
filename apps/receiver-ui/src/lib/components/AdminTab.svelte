<script lang="ts">
  import { onMount } from "svelte";
  import { HelpTip } from "@rusty-timer/shared-ui";
  import * as api from "$lib/api";
  import { loadAll as globalLoadAll } from "$lib/store.svelte";

  let streams = $state<api.StreamEntry[]>([]);
  let subscriptions = $state<api.SubscriptionItem[]>([]);
  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let inFlightKeys = $state<Set<string>>(new Set());
  let inFlightAction = $state<string | null>(null);
  let feedback = $state<{ message: string; ok: boolean } | null>(null);
  let confirmingClearData = $state(false);
  let confirmingFactoryReset = $state(false);
  let portEdits = $state<Map<string, string>>(new Map());

  function streamKey(s: { forwarder_id: string; reader_ip: string }): string {
    return `${s.forwarder_id}/${s.reader_ip}`;
  }

  function streamLabel(s: api.StreamEntry): string {
    return s.display_alias ?? `${s.forwarder_id} / ${s.reader_ip}`;
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
      // Also refresh global store state so other tabs see the changes.
      void globalLoadAll();
    } catch {
      setFeedback(`${label}: failed.`, false);
    } finally {
      inFlightAction = null;
    }
  }

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
    return portEdits.get(key)! !== (sub.local_port_override?.toString() ?? "");
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
        portValue !== null
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

  async function handleClearData() {
    confirmingClearData = false;
    await handleBulkAction(() => api.clearData(), "Clear data", "clear-data");
  }

  async function handleFactoryReset() {
    confirmingFactoryReset = false;
    await handleBulkAction(
      () => api.factoryReset(),
      "Factory reset",
      "factory-reset",
    );
  }

  const btnWarn =
    "px-2.5 py-1 text-xs font-medium rounded-md text-status-warn border border-status-warn-border bg-status-warn-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed";
  const btnDanger =
    "px-3 py-1.5 text-xs font-medium rounded-md text-status-err border border-status-err bg-transparent cursor-pointer hover:opacity-80";
  const btnDangerConfirm =
    "px-3 py-1.5 text-xs font-medium rounded-md text-white bg-status-err border border-status-err cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed";
  const btnNeutral =
    "px-2.5 py-1 text-xs font-medium rounded-md text-text-primary border border-border bg-surface-2 cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed";

  onMount(loadAll);
</script>

<div class="max-w-[700px] mx-auto px-6 py-6">
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
      <section>
        <h3 class="text-sm font-semibold text-text-primary mb-1">
          Cursor Reset
        </h3>
        <p class="text-xs text-text-muted m-0 mb-3">
          Reset resume cursors per stream. The selected stream will replay from
          the beginning on next connect.
        </p>
        {#if streams.length === 0}
          <p class="text-xs text-text-muted m-0">No streams available.</p>
        {:else}
          <table class="w-full text-sm">
            <thead>
              <tr class="border-b border-border text-left text-text-muted">
                <th class="py-1.5 pr-3 font-medium text-xs">Stream</th>
                <th class="py-1.5 pr-3 font-medium text-xs">Epoch</th>
                <th class="py-1.5 pr-3 font-medium text-xs">Seq</th>
                <th class="py-1.5 font-medium text-xs"></th>
              </tr>
            </thead>
            <tbody>
              {#each streams as stream (streamKey(stream))}
                {@const key = streamKey(stream)}
                <tr class="border-b border-border/50">
                  <td class="py-1.5 pr-3 text-text-primary text-xs">
                    {streamLabel(stream)}
                    <span class="block text-text-muted font-mono"
                      >{stream.reader_ip}</span
                    >
                  </td>
                  <td class="py-1.5 pr-3 text-text-muted tabular-nums text-xs"
                    >{stream.cursor_epoch ?? "\u2014"}</td
                  >
                  <td class="py-1.5 pr-3 text-text-muted tabular-nums text-xs"
                    >{stream.cursor_seq ?? "\u2014"}</td
                  >
                  <td class="py-1.5 text-right">
                    <button
                      onclick={() => handleResetCursor(stream)}
                      disabled={inFlightKeys.has(key)}
                      class={btnWarn}
                    >
                      {inFlightKeys.has(key) ? "Resetting..." : "Reset Cursor"}
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
          <div class="mt-3 flex justify-end">
            <button
              onclick={() =>
                handleBulkAction(
                  () => api.resetAllCursors(),
                  "Reset all cursors",
                  "reset-all-cursors",
                )}
              disabled={inFlightAction === "reset-all-cursors"}
              class={btnWarn}
            >
              {inFlightAction === "reset-all-cursors"
                ? "Resetting..."
                : "Reset All Cursors"}
            </button>
          </div>
        {/if}
      </section>

      <hr class="border-border" />

      <!-- Earliest-Epoch Overrides -->
      <section>
        <h3 class="text-sm font-semibold text-text-primary mb-1">
          Earliest-Epoch Overrides
        </h3>
        <p class="text-xs text-text-muted m-0 mb-3">
          Clear earliest-epoch overrides per stream or all at once.
        </p>
        {#if streams.length === 0}
          <p class="text-xs text-text-muted m-0">No streams available.</p>
        {:else}
          <table class="w-full text-sm">
            <thead>
              <tr class="border-b border-border text-left text-text-muted">
                <th class="py-1.5 pr-3 font-medium text-xs">Stream</th>
                <th class="py-1.5 font-medium text-xs"></th>
              </tr>
            </thead>
            <tbody>
              {#each streams as stream (streamKey(stream))}
                {@const key = `epoch-${streamKey(stream)}`}
                <tr class="border-b border-border/50">
                  <td class="py-1.5 pr-3 text-text-primary text-xs"
                    >{streamLabel(stream)}</td
                  >
                  <td class="py-1.5 text-right">
                    <button
                      onclick={() => handleResetEpoch(stream)}
                      disabled={inFlightKeys.has(key)}
                      class={btnWarn}
                    >
                      {inFlightKeys.has(key) ? "Resetting..." : "Reset Epoch"}
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
          <div class="mt-3 flex justify-end">
            <button
              onclick={() =>
                handleBulkAction(
                  () => api.resetAllEarliestEpochs(),
                  "Reset all earliest-epoch overrides",
                  "reset-all-epochs",
                )}
              disabled={inFlightAction === "reset-all-epochs"}
              class={btnWarn}
            >
              {inFlightAction === "reset-all-epochs"
                ? "Resetting..."
                : "Reset All Epoch Overrides"}
            </button>
          </div>
        {/if}
      </section>

      <hr class="border-border" />

      <!-- Local Port Overrides -->
      <section>
        <h3 class="text-sm font-semibold text-text-primary mb-1">
          Local Port Overrides
          <HelpTip
            fieldKey="port_override"
            sectionKey="port_overrides"
            context="receiver-admin"
          />
        </h3>
        <p class="text-xs text-text-muted m-0 mb-3">
          Set or clear the local forwarding port per subscription.
        </p>
        {#if subscriptions.length === 0}
          <p class="text-xs text-text-muted m-0">No subscriptions.</p>
        {:else}
          <table class="w-full text-sm">
            <thead>
              <tr class="border-b border-border text-left text-text-muted">
                <th class="py-1.5 pr-3 font-medium text-xs">Forwarder</th>
                <th class="py-1.5 pr-3 font-medium text-xs">Reader</th>
                <th class="py-1.5 pr-3 font-medium text-xs">Port</th>
                <th class="py-1.5 font-medium text-xs"></th>
              </tr>
            </thead>
            <tbody>
              {#each subscriptions as sub (streamKey(sub))}
                {@const portKey = `port-${streamKey(sub)}`}
                <tr class="border-b border-border/50">
                  <td class="py-1.5 pr-3 text-text-muted text-xs"
                    >{sub.forwarder_id}</td
                  >
                  <td class="py-1.5 pr-3 text-text-muted text-xs"
                    >{sub.reader_ip}</td
                  >
                  <td class="py-1.5 pr-3">
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
                      class="w-20 px-2 py-0.5 text-xs rounded border border-border bg-surface-0 text-text-primary font-mono"
                    />
                  </td>
                  <td class="py-1.5 text-right">
                    <button
                      onclick={() => handleSavePort(sub)}
                      disabled={!isPortDirty(sub) || inFlightKeys.has(portKey)}
                      class={btnNeutral}
                    >
                      {inFlightKeys.has(portKey) ? "Saving..." : "Save"}
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </section>

      <hr class="border-border" />

      <!-- Purge Subscriptions -->
      <section>
        <h3 class="text-sm font-semibold text-text-primary mb-1">
          Purge Subscriptions
        </h3>
        <p class="text-xs text-text-muted m-0 mb-3">
          Remove all stream subscriptions.
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
          class={btnWarn}
        >
          {inFlightAction === "purge-subs"
            ? "Purging..."
            : "Purge All Subscriptions"}
        </button>
      </section>

      <hr class="border-border" />

      <!-- Reset Profile -->
      <section>
        <h3 class="text-sm font-semibold text-text-primary mb-1">
          Reset Profile
        </h3>
        <p class="text-xs text-text-muted m-0 mb-3">
          Clear server URL, token, and receiver ID back to defaults.
        </p>
        <button
          onclick={() =>
            handleBulkAction(
              () => api.resetProfile(),
              "Reset profile",
              "reset-profile",
            )}
          disabled={inFlightAction === "reset-profile"}
          class={btnWarn}
        >
          {inFlightAction === "reset-profile"
            ? "Resetting..."
            : "Reset Profile to Defaults"}
        </button>
      </section>

      <hr class="border-border" />

      <!-- Clear Data -->
      <section>
        <h3 class="text-sm font-semibold text-status-err mb-1">Clear Data</h3>
        <p class="text-xs text-text-muted m-0 mb-3">
          Clear all subscriptions, cursors, races, mode, and DBF config. Keeps
          server URL, token, and receiver ID.
        </p>
        {#if confirmingClearData}
          <div class="flex items-center gap-3">
            <span class="text-sm text-status-err font-medium"
              >Are you sure?</span
            >
            <button
              onclick={handleClearData}
              disabled={inFlightAction === "clear-data"}
              class={btnDangerConfirm}
            >
              {inFlightAction === "clear-data"
                ? "Clearing..."
                : "Yes, Clear Data"}
            </button>
            <button
              onclick={() => (confirmingClearData = false)}
              class={btnNeutral}
            >
              Cancel
            </button>
          </div>
        {:else}
          <button
            onclick={() => (confirmingClearData = true)}
            class={btnDanger}
          >
            Clear Data...
          </button>
        {/if}
      </section>

      <hr class="border-border" />

      <!-- Factory Reset -->
      <section>
        <h3 class="text-sm font-semibold text-status-err mb-1">
          Factory Reset
        </h3>
        <p class="text-xs text-text-muted m-0 mb-3">
          Clear <strong>all</strong> local data. This cannot be undone.
        </p>
        {#if confirmingFactoryReset}
          <div class="flex items-center gap-3">
            <span class="text-sm text-status-err font-medium"
              >Are you sure?</span
            >
            <button
              onclick={handleFactoryReset}
              disabled={inFlightAction === "factory-reset"}
              class={btnDangerConfirm}
            >
              {inFlightAction === "factory-reset"
                ? "Resetting..."
                : "Yes, Factory Reset"}
            </button>
            <button
              onclick={() => (confirmingFactoryReset = false)}
              class={btnNeutral}
            >
              Cancel
            </button>
          </div>
        {:else}
          <button
            onclick={() => (confirmingFactoryReset = true)}
            class={btnDanger}
          >
            Factory Reset...
          </button>
        {/if}
      </section>
    </div>
  {/if}
</div>
