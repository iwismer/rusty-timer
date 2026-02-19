<script lang="ts">
  import { onMount } from "svelte";
  import type { ConfigApi } from "../lib/config-types";
  import Card from "./Card.svelte";
  import AlertBanner from "./AlertBanner.svelte";
  import StatusBadge from "./StatusBadge.svelte";

  export let configApi: ConfigApi;
  export let displayName: string | undefined = undefined;
  export let isOnline: boolean | undefined = undefined;

  let loading = true;
  let loadError: string | null = null;
  let configLoaded = false;
  let restartNeeded = false;
  let sectionMessages: Record<string, { ok: boolean; text: string }> = {};
  let savingSection: Record<string, boolean> = {};
  let restarting = false;
  let restartMessage: { ok: boolean; text: string } | null = null;

  // Form fields
  let generalDisplayName = "";
  let serverBaseUrl = "";
  let serverForwardersWsPath = "";
  let authTokenFile = "";
  let journalSqlitePath = "";
  let journalPruneWatermarkPct = "";
  let uplinkBatchMode = "";
  let uplinkBatchFlushMs = "";
  let uplinkBatchMaxEvents = "";
  let statusHttpBind = "";

  interface ReaderEntry {
    target: string;
    enabled: boolean;
    local_fallback_port: string;
  }
  let readers: ReaderEntry[] = [];

  onMount(async () => {
    await loadConfig();
  });

  async function loadConfig() {
    loading = true;
    loadError = null;
    try {
      const result = await configApi.getConfig();
      if (!result.ok) {
        throw new Error(result.error ?? "Failed to load config");
      }
      restartNeeded = result.restart_needed;
      populateFields(result.config);
      configLoaded = true;
    } catch (e) {
      loadError = String(e);
    } finally {
      loading = false;
    }
  }

  function populateFields(cfg: Record<string, unknown>) {
    generalDisplayName = (cfg.display_name as string) ?? "";

    const server = (cfg.server as Record<string, unknown>) ?? {};
    serverBaseUrl = (server.base_url as string) ?? "";
    serverForwardersWsPath = (server.forwarders_ws_path as string) ?? "";

    const auth = (cfg.auth as Record<string, unknown>) ?? {};
    authTokenFile = (auth.token_file as string) ?? "";

    const journal = (cfg.journal as Record<string, unknown>) ?? {};
    journalSqlitePath = (journal.sqlite_path as string) ?? "";
    journalPruneWatermarkPct =
      journal.prune_watermark_pct != null
        ? String(journal.prune_watermark_pct)
        : "";

    const uplink = (cfg.uplink as Record<string, unknown>) ?? {};
    uplinkBatchMode = (uplink.batch_mode as string) ?? "";
    uplinkBatchFlushMs =
      uplink.batch_flush_ms != null ? String(uplink.batch_flush_ms) : "";
    uplinkBatchMaxEvents =
      uplink.batch_max_events != null ? String(uplink.batch_max_events) : "";

    const statusHttp = (cfg.status_http as Record<string, unknown>) ?? {};
    statusHttpBind = (statusHttp.bind as string) ?? "";

    const rawReaders = (cfg.readers as Record<string, unknown>[]) ?? [];
    readers = rawReaders.map((r) => ({
      target: (r.target as string) ?? "",
      enabled: (r.enabled as boolean) ?? true,
      local_fallback_port:
        r.local_fallback_port != null ? String(r.local_fallback_port) : "",
    }));
  }

  async function saveSection(
    section: string,
    payload: Record<string, unknown>,
  ) {
    sectionMessages[section] = { ok: false, text: "Saving..." };
    savingSection[section] = true;
    try {
      const result = await configApi.saveSection(section, payload);
      if (result.ok) {
        sectionMessages[section] = {
          ok: true,
          text: "Saved. Restart to apply.",
        };
        restartNeeded = result.restart_needed;
      } else {
        sectionMessages[section] = {
          ok: false,
          text: result.error ?? "Unknown error",
        };
      }
    } catch (e) {
      sectionMessages[section] = { ok: false, text: String(e) };
    } finally {
      savingSection[section] = false;
    }
  }

  function saveGeneral() {
    saveSection("general", { display_name: generalDisplayName || null });
  }
  function saveServer() {
    saveSection("server", {
      base_url: serverBaseUrl,
      forwarders_ws_path: serverForwardersWsPath || null,
    });
  }
  function saveAuth() {
    saveSection("auth", { token_file: authTokenFile });
  }
  function saveJournal() {
    saveSection("journal", {
      sqlite_path: journalSqlitePath || null,
      prune_watermark_pct: journalPruneWatermarkPct
        ? Number(journalPruneWatermarkPct)
        : null,
    });
  }
  function saveUplink() {
    saveSection("uplink", {
      batch_mode: uplinkBatchMode || null,
      batch_flush_ms: uplinkBatchFlushMs ? Number(uplinkBatchFlushMs) : null,
      batch_max_events: uplinkBatchMaxEvents
        ? Number(uplinkBatchMaxEvents)
        : null,
    });
  }
  function saveStatusHttp() {
    saveSection("status_http", { bind: statusHttpBind || null });
  }
  function saveReaders() {
    saveSection("readers", {
      readers: readers.map((r) => ({
        target: r.target || null,
        enabled: r.enabled,
        local_fallback_port: r.local_fallback_port
          ? Number(r.local_fallback_port)
          : null,
      })),
    });
  }

  function addReader() {
    readers = [
      ...readers,
      { target: "", enabled: true, local_fallback_port: "" },
    ];
  }
  function removeReader(index: number) {
    readers = readers.filter((_, i) => i !== index);
  }

  async function doRestart() {
    restarting = true;
    restartMessage = null;
    try {
      const result = await configApi.restart();
      if (result.ok) {
        restartMessage = { ok: true, text: "Restart initiated." };
        restartNeeded = false;
      } else {
        restartMessage = {
          ok: false,
          text: result.error ?? "Unknown error",
        };
      }
    } catch (e) {
      restartMessage = { ok: false, text: String(e) };
    } finally {
      restarting = false;
    }
  }

  const inputClass =
    "w-full px-2 py-1.5 text-sm rounded-md border border-border bg-surface-0 text-text-primary placeholder:text-text-muted focus:outline-none focus:border-accent";
  const saveBtnClass =
    "mt-2 px-3 py-1.5 text-xs font-medium rounded-md bg-accent text-white border-none cursor-pointer hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed";
</script>

<div>
  {#if displayName !== undefined}
    <div class="flex items-center gap-3 mb-4">
      <h1 class="text-lg font-bold text-text-primary m-0">
        Configure: {displayName}
      </h1>
      {#if isOnline !== undefined}
        <StatusBadge
          label={isOnline ? "online" : "offline"}
          state={isOnline ? "ok" : "err"}
        />
      {/if}
    </div>
  {/if}

  {#if restartNeeded}
    <div class="mb-4">
      <AlertBanner
        variant="warn"
        message="Restart needed — some changes require a forwarder restart to take effect."
        actionLabel={restarting ? "Restarting..." : "Restart Now"}
        actionBusy={restarting}
        onAction={doRestart}
      />
    </div>
  {/if}

  {#if restartMessage}
    <p
      class="text-sm mb-4 m-0 {restartMessage.ok
        ? 'text-status-ok'
        : 'text-status-err'}"
    >
      {restartMessage.text}
    </p>
  {/if}

  {#if loading}
    <p class="text-sm text-text-muted">Loading configuration...</p>
  {:else if loadError}
    <div class="mb-4">
      <AlertBanner variant="err" message={loadError} actionLabel="Retry" onAction={loadConfig} />
    </div>
  {:else if configLoaded}
    <div class="space-y-4">
      <!-- General -->
      <Card title="General">
        <label class="block text-sm font-medium text-text-secondary mb-1">
          Display Name
        </label>
        <input type="text" bind:value={generalDisplayName} class={inputClass} />
        <button
          class={saveBtnClass}
          on:click={saveGeneral}
          disabled={savingSection["general"]}
        >
          {savingSection["general"] ? "Saving..." : "Save General"}
        </button>
        {#if sectionMessages["general"]}
          <p
            class="text-xs mt-1 m-0 {sectionMessages['general'].ok
              ? 'text-status-ok'
              : 'text-status-err'}"
          >
            {sectionMessages["general"].text}
          </p>
        {/if}
      </Card>

      <!-- Server -->
      <Card title="Server">
        <div class="space-y-3">
          <div>
            <label class="block text-sm font-medium text-text-secondary mb-1">
              Base URL *
            </label>
            <input
              type="text"
              bind:value={serverBaseUrl}
              required
              class={inputClass}
            />
          </div>
          <div>
            <label class="block text-sm font-medium text-text-secondary mb-1">
              Forwarders WS Path
            </label>
            <input
              type="text"
              bind:value={serverForwardersWsPath}
              class={inputClass}
            />
          </div>
        </div>
        <button
          class={saveBtnClass}
          on:click={saveServer}
          disabled={savingSection["server"]}
        >
          {savingSection["server"] ? "Saving..." : "Save Server"}
        </button>
        {#if sectionMessages["server"]}
          <p
            class="text-xs mt-1 m-0 {sectionMessages['server'].ok
              ? 'text-status-ok'
              : 'text-status-err'}"
          >
            {sectionMessages["server"].text}
          </p>
        {/if}
      </Card>

      <!-- Auth -->
      <Card title="Auth">
        <label class="block text-sm font-medium text-text-secondary mb-1">
          Token File Path *
        </label>
        <input
          type="text"
          bind:value={authTokenFile}
          required
          class={inputClass}
        />
        <button
          class={saveBtnClass}
          on:click={saveAuth}
          disabled={savingSection["auth"]}
        >
          {savingSection["auth"] ? "Saving..." : "Save Auth"}
        </button>
        {#if sectionMessages["auth"]}
          <p
            class="text-xs mt-1 m-0 {sectionMessages['auth'].ok
              ? 'text-status-ok'
              : 'text-status-err'}"
          >
            {sectionMessages["auth"].text}
          </p>
        {/if}
      </Card>

      <!-- Journal -->
      <Card title="Journal">
        <div class="space-y-3">
          <div>
            <label class="block text-sm font-medium text-text-secondary mb-1">
              SQLite Path
            </label>
            <input
              type="text"
              bind:value={journalSqlitePath}
              class={inputClass}
            />
          </div>
          <div>
            <label class="block text-sm font-medium text-text-secondary mb-1">
              Prune Watermark %
            </label>
            <input
              type="number"
              bind:value={journalPruneWatermarkPct}
              min="0"
              max="100"
              class={inputClass}
            />
          </div>
        </div>
        <button
          class={saveBtnClass}
          on:click={saveJournal}
          disabled={savingSection["journal"]}
        >
          {savingSection["journal"] ? "Saving..." : "Save Journal"}
        </button>
        {#if sectionMessages["journal"]}
          <p
            class="text-xs mt-1 m-0 {sectionMessages['journal'].ok
              ? 'text-status-ok'
              : 'text-status-err'}"
          >
            {sectionMessages["journal"].text}
          </p>
        {/if}
      </Card>

      <!-- Uplink -->
      <Card title="Uplink">
        <div class="space-y-3">
          <div>
            <label class="block text-sm font-medium text-text-secondary mb-1">
              Batch Mode
            </label>
            <input
              type="text"
              bind:value={uplinkBatchMode}
              class={inputClass}
            />
          </div>
          <div>
            <label class="block text-sm font-medium text-text-secondary mb-1">
              Batch Flush (ms)
            </label>
            <input
              type="number"
              bind:value={uplinkBatchFlushMs}
              min="0"
              class={inputClass}
            />
          </div>
          <div>
            <label class="block text-sm font-medium text-text-secondary mb-1">
              Batch Max Events
            </label>
            <input
              type="number"
              bind:value={uplinkBatchMaxEvents}
              min="0"
              class={inputClass}
            />
          </div>
        </div>
        <button
          class={saveBtnClass}
          on:click={saveUplink}
          disabled={savingSection["uplink"]}
        >
          {savingSection["uplink"] ? "Saving..." : "Save Uplink"}
        </button>
        {#if sectionMessages["uplink"]}
          <p
            class="text-xs mt-1 m-0 {sectionMessages['uplink'].ok
              ? 'text-status-ok'
              : 'text-status-err'}"
          >
            {sectionMessages["uplink"].text}
          </p>
        {/if}
      </Card>

      <!-- Status HTTP -->
      <Card title="Status HTTP">
        <label class="block text-sm font-medium text-text-secondary mb-1">
          Bind Address
        </label>
        <input type="text" bind:value={statusHttpBind} class={inputClass} />
        <button
          class={saveBtnClass}
          on:click={saveStatusHttp}
          disabled={savingSection["status_http"]}
        >
          {savingSection["status_http"] ? "Saving..." : "Save Status HTTP"}
        </button>
        {#if sectionMessages["status_http"]}
          <p
            class="text-xs mt-1 m-0 {sectionMessages['status_http'].ok
              ? 'text-status-ok'
              : 'text-status-err'}"
          >
            {sectionMessages["status_http"].text}
          </p>
        {/if}
      </Card>

      <!-- Readers -->
      <Card title="Readers">
        <div class="overflow-x-auto">
          <table class="w-full text-sm border-collapse">
            <thead>
              <tr class="border-b-2 border-border">
                <th class="text-left py-2 px-2 text-xs font-medium text-text-muted">Target *</th>
                <th class="text-left py-2 px-2 text-xs font-medium text-text-muted">Enabled</th>
                <th class="text-left py-2 px-2 text-xs font-medium text-text-muted">Fallback Port</th>
                <th class="py-2 px-2"></th>
              </tr>
            </thead>
            <tbody>
              {#each readers as reader, i}
                <tr class="border-b border-border">
                  <td class="py-1.5 px-2">
                    <input
                      type="text"
                      bind:value={reader.target}
                      required
                      placeholder="192.168.0.50:10000"
                      class={inputClass}
                    />
                  </td>
                  <td class="py-1.5 px-2">
                    <input
                      type="checkbox"
                      bind:checked={reader.enabled}
                      class="accent-accent"
                    />
                  </td>
                  <td class="py-1.5 px-2">
                    <input
                      type="number"
                      bind:value={reader.local_fallback_port}
                      min="1"
                      max="65535"
                      class={inputClass}
                    />
                  </td>
                  <td class="py-1.5 px-2">
                    <button
                      on:click={() => removeReader(i)}
                      class="px-2 py-1 text-xs rounded-md text-status-err border border-status-err-border bg-status-err-bg cursor-pointer hover:opacity-80"
                    >
                      Remove
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
        <div class="flex gap-2 mt-2">
          <button
            on:click={addReader}
            class="px-3 py-1.5 text-xs font-medium rounded-md bg-surface-2 border border-border text-text-secondary cursor-pointer hover:bg-surface-3"
          >
            + Add Reader
          </button>
          <button
            class={saveBtnClass}
            on:click={saveReaders}
            disabled={savingSection["readers"]}
          >
            {savingSection["readers"] ? "Saving..." : "Save Readers"}
          </button>
        </div>
        {#if sectionMessages["readers"]}
          <p
            class="text-xs mt-1 m-0 {sectionMessages['readers'].ok
              ? 'text-status-ok'
              : 'text-status-err'}"
          >
            {sectionMessages["readers"].text}
          </p>
        {/if}
      </Card>
    </div>
  {/if}
</div>
