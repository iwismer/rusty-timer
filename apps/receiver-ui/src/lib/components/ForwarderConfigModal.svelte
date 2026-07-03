<script lang="ts">
  import { Card } from "@rusty-timer/shared-ui";
  import {
    getForwarderConfig,
    restartForwarder,
    setForwarderConfig,
  } from "$lib/api";
  import { btnPrimary, btnSecondary, inputClass } from "$lib/ui-classes";

  let {
    open,
    endpointId,
    displayName = null,
    onClose,
  }: {
    open: boolean;
    endpointId: string | null;
    displayName?: string | null;
    onClose: () => void;
  } = $props();

  let config: Record<string, any> | null = $state(null);
  let loading = $state(false);
  let saving = $state(false);
  let restarting = $state(false);
  let loadedEndpointId: string | null = $state(null);
  let restartNeeded = $state(false);
  let loadError: string | null = $state(null);
  let actionError: string | null = $state(null);
  let actionMessage: string | null = $state(null);

  $effect(() => {
    if (!open) {
      loadedEndpointId = null;
      return;
    }
    if (endpointId && loadedEndpointId !== endpointId && !loading) {
      void loadConfig(endpointId);
    }
  });

  function isRecord(value: unknown): value is Record<string, any> {
    return value !== null && typeof value === "object" && !Array.isArray(value);
  }

  function ensureRecord(
    parent: Record<string, any>,
    key: string,
  ): Record<string, any> {
    if (!isRecord(parent[key])) {
      parent[key] = {};
    }
    return parent[key];
  }

  function ensureArray(
    parent: Record<string, any>,
    key: string,
  ): Record<string, any>[] {
    if (!Array.isArray(parent[key])) {
      parent[key] = [];
    }
    return parent[key];
  }

  function normalizeEditableFields(parsed: Record<string, any>): void {
    ensureRecord(parsed, "p2p");
    ensureRecord(parsed, "journal");
    ensureRecord(parsed, "status_http");
    ensureRecord(parsed, "control");
    ensureRecord(parsed, "update");

    const readers = ensureArray(parsed, "readers");
    for (let i = 0; i < readers.length; i += 1) {
      if (!isRecord(readers[i])) {
        readers[i] = {};
      }
    }
  }

  async function loadConfig(targetEndpointId: string): Promise<void> {
    loading = true;
    loadError = null;
    actionError = null;
    actionMessage = null;
    config = null;
    loadedEndpointId = targetEndpointId;

    try {
      const response = await getForwarderConfig(targetEndpointId);
      let parsed: unknown;
      try {
        parsed = JSON.parse(response.config_json) as unknown;
      } catch {
        throw new Error("Failed to read forwarder config (invalid response).");
      }
      if (!isRecord(parsed)) {
        throw new Error("Forwarder config must be a JSON object");
      }
      normalizeEditableFields(parsed);
      config = parsed;
      restartNeeded = response.restart_needed;
    } catch (e) {
      loadError = String(e);
    } finally {
      loading = false;
    }
  }

  // Build the document to send. Preserves the full parsed config (so untouched
  // and unknown fields survive the round-trip) but drops the synthesized empty
  // `update.mode` produced by the "Default" select option — the forwarder
  // rejects an empty mode string, and an absent mode means "use the default".
  function buildConfigPayload(
    source: Record<string, any>,
  ): Record<string, any> {
    const payload = JSON.parse(JSON.stringify(source)) as Record<string, any>;
    if (isRecord(payload.update) && payload.update.mode === "") {
      delete payload.update.mode;
    }
    return payload;
  }

  async function saveConfig(): Promise<void> {
    if (!endpointId || !config) return;

    saving = true;
    actionError = null;
    actionMessage = null;

    try {
      const result = await setForwarderConfig(
        endpointId,
        JSON.stringify(buildConfigPayload(config)),
      );
      if (!result.ok) {
        actionError = result.error || "Failed to save forwarder config";
        return;
      }
      restartNeeded = result.restart_needed;
      actionMessage = result.restart_needed
        ? "Config saved. Restart the forwarder to apply all changes."
        : "Config saved.";
    } catch (e) {
      actionError = String(e);
    } finally {
      saving = false;
    }
  }

  async function restart(): Promise<void> {
    if (!endpointId) return;

    restarting = true;
    actionError = null;
    actionMessage = null;

    try {
      const result = await restartForwarder(endpointId);
      if (!result.accepted) {
        actionError = result.error || "Forwarder restart was not accepted";
        return;
      }
      actionMessage = "Forwarder restart accepted.";
    } catch (e) {
      actionError = String(e);
    } finally {
      restarting = false;
    }
  }
</script>

{#if open}
  <div
    class="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/60 px-4 py-8"
    role="dialog"
    aria-modal="true"
    aria-label="Forwarder configuration"
    data-testid="forwarder-config-modal"
  >
    <div
      class="w-full max-w-4xl rounded-lg border border-border bg-surface-0 shadow-xl"
    >
      <div
        class="flex items-start justify-between gap-4 border-b border-border px-5 py-4"
      >
        <div>
          <h2 class="m-0 text-lg font-semibold text-text-primary">
            Configure {displayName ?? endpointId}
          </h2>
          {#if endpointId}
            <p class="mt-1 font-mono text-xs text-text-muted">{endpointId}</p>
          {/if}
        </div>
        <button class={btnSecondary} type="button" onclick={onClose}
          >Close</button
        >
      </div>

      <div class="space-y-4 px-5 py-4">
        {#if loading}
          <p class="text-sm text-text-muted">Loading configuration…</p>
        {:else if loadError}
          <p
            data-testid="forwarder-config-error"
            class="text-sm text-status-err"
          >
            {loadError}
          </p>
        {:else if config}
          {#if restartNeeded}
            <div
              data-testid="forwarder-config-restart-banner"
              class="rounded-md border border-status-warn-border bg-status-warn-bg p-3 text-sm text-status-warn"
            >
              <div class="flex flex-wrap items-center justify-between gap-3">
                <span
                  >Restart needed — restart the forwarder to apply all changes.</span
                >
                <button
                  data-testid="forwarder-config-restart"
                  class={btnPrimary}
                  type="button"
                  onclick={() => void restart()}
                  disabled={restarting}
                >
                  {restarting ? "Restarting…" : "Restart"}
                </button>
              </div>
            </div>
          {/if}

          {#if actionError}
            <p
              data-testid="forwarder-config-error"
              class="text-sm text-status-err"
            >
              {actionError}
            </p>
          {/if}
          {#if actionMessage}
            <p class="text-sm text-status-ok">{actionMessage}</p>
          {/if}

          <div class="grid grid-cols-1 gap-4 md:grid-cols-2">
            <Card title="General">
              <label class="block text-sm font-medium text-text-secondary">
                Display name
                <input
                  class="mt-1 {inputClass}"
                  type="text"
                  bind:value={config.display_name}
                  aria-label="Display name"
                />
              </label>
            </Card>

            <Card title="P2P / Server">
              <div class="space-y-3">
                <p class="text-xs text-text-muted">
                  Managed locally on the forwarder — not editable remotely.
                </p>
                <label class="block text-sm font-medium text-text-secondary">
                  <span class="inline-flex items-center gap-2">
                    <input
                      class="accent-accent"
                      type="checkbox"
                      disabled
                      bind:checked={config.p2p.enabled}
                    />
                    Enable P2P
                  </span>
                </label>
                <label class="block text-sm font-medium text-text-secondary">
                  Server URL
                  <input
                    class="mt-1 {inputClass} opacity-50"
                    type="text"
                    disabled
                    bind:value={config.p2p.server_url}
                  />
                </label>
                <label class="block text-sm font-medium text-text-secondary">
                  Server token file
                  <input
                    class="mt-1 {inputClass} opacity-50"
                    type="text"
                    disabled
                    bind:value={config.p2p.server_token_file}
                  />
                </label>
              </div>
            </Card>

            <Card title="Journal">
              <div class="space-y-3">
                <label class="block text-sm font-medium text-text-secondary">
                  SQLite path
                  <input
                    class="mt-1 {inputClass}"
                    type="text"
                    bind:value={config.journal.sqlite_path}
                  />
                </label>
                <label class="block text-sm font-medium text-text-secondary">
                  Prune watermark %
                  <input
                    class="mt-1 {inputClass}"
                    type="number"
                    min="0"
                    max="100"
                    bind:value={config.journal.prune_watermark_pct}
                  />
                </label>
              </div>
            </Card>

            <Card title="Status HTTP">
              <label class="block text-sm font-medium text-text-secondary">
                Bind address
                <input
                  class="mt-1 {inputClass}"
                  type="text"
                  bind:value={config.status_http.bind}
                />
              </label>
            </Card>

            <Card title="Control">
              <div class="space-y-3">
                <p class="text-xs text-text-muted">
                  Managed locally on the forwarder — not editable remotely.
                </p>
                <label class="block text-sm font-medium text-text-secondary">
                  <span class="inline-flex items-center gap-2">
                    <input
                      class="accent-accent"
                      type="checkbox"
                      disabled
                      bind:checked={config.control.allow_power_actions}
                    />
                    Allow power actions
                  </span>
                </label>
                <label class="block text-sm font-medium text-text-secondary">
                  <span class="inline-flex items-center gap-2">
                    <input
                      class="accent-accent"
                      type="checkbox"
                      disabled
                      bind:checked={config.control.allow_remote_config}
                    />
                    Allow remote config
                  </span>
                </label>
              </div>
            </Card>

            <Card title="Update">
              <label class="block text-sm font-medium text-text-secondary">
                Update mode
                <select
                  class="mt-1 {inputClass}"
                  bind:value={config.update.mode}
                >
                  <option value="">Default</option>
                  <option value="check-and-download">Automatic</option>
                  <option value="check-only">Check only</option>
                  <option value="disabled">Disabled</option>
                </select>
              </label>
            </Card>
          </div>

          <Card title="Readers">
            {#if config.readers.length === 0}
              <p class="text-sm text-text-muted">No readers configured.</p>
            {:else}
              <div class="space-y-3">
                {#each config.readers as reader, i}
                  <div
                    class="grid grid-cols-1 gap-3 rounded-md border border-border bg-surface-1 p-3 md:grid-cols-[1fr_auto_10rem]"
                  >
                    <label
                      class="block text-sm font-medium text-text-secondary"
                    >
                      Target
                      <input
                        class="mt-1 {inputClass}"
                        type="text"
                        bind:value={reader.target}
                        aria-label={`Reader ${i + 1} target`}
                      />
                    </label>
                    <label
                      class="flex items-center gap-2 text-sm font-medium text-text-secondary"
                    >
                      <input
                        class="accent-accent"
                        type="checkbox"
                        bind:checked={reader.enabled}
                        aria-label={`Reader ${i + 1} enabled`}
                      />
                      Enabled
                    </label>
                    <label
                      class="block text-sm font-medium text-text-secondary"
                    >
                      Local fallback port
                      <input
                        class="mt-1 {inputClass}"
                        type="number"
                        min="1"
                        max="65535"
                        bind:value={reader.local_fallback_port}
                        aria-label={`Reader ${i + 1} local fallback port`}
                      />
                    </label>
                  </div>
                {/each}
              </div>
            {/if}
          </Card>

          <div class="flex justify-end gap-2 border-t border-border pt-4">
            <button class={btnSecondary} type="button" onclick={onClose}
              >Cancel</button
            >
            <button
              data-testid="forwarder-config-save"
              class={btnPrimary}
              type="button"
              onclick={() => void saveConfig()}
              disabled={saving}
            >
              {saving ? "Saving…" : "Save"}
            </button>
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}
