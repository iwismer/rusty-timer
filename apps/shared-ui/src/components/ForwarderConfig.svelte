<script lang="ts">
  import { onMount } from "svelte";
  import type { ConfigApi } from "../lib/config-types";
  import {
    fromConfig,
    toGeneralPayload,
    toServerPayload,
    toAuthPayload,
    toJournalPayload,
    toUplinkPayload,
    toStatusHttpPayload,
    toReadersPayload,
    type ReaderEntry,
    type ForwarderConfigFormState,
  } from "../lib/forwarder-config-form";
  import { saveSuccessMessage } from "../lib/forwarder-config-logic";
  import Card from "./Card.svelte";
  import AlertBanner from "./AlertBanner.svelte";
  import StatusBadge from "./StatusBadge.svelte";

  let {
    configApi,
    displayName = undefined,
    isOnline = undefined,
  }: {
    configApi: ConfigApi;
    displayName?: string;
    isOnline?: boolean;
  } = $props();

  let loading = $state(true);
  let loadError: string | null = $state(null);
  let configLoaded = $state(false);
  let restartNeeded = $state(false);
  let sectionMessages: Record<string, { ok: boolean; text: string }> = $state({});
  let savingSection: Record<string, boolean> = $state({});
  let restarting = $state(false);
  let restartMessage: { ok: boolean; text: string } | null = $state(null);

  // Form fields
  let generalDisplayName = $state("");
  let serverBaseUrl = $state("");
  let serverForwardersWsPath = $state("");
  let authTokenFile = $state("");
  let journalSqlitePath = $state("");
  let journalPruneWatermarkPct = $state("");
  let uplinkBatchMode = $state("");
  let uplinkBatchFlushMs = $state("");
  let uplinkBatchMaxEvents = $state("");
  let statusHttpBind = $state("");
  let readers: ReaderEntry[] = $state([]);

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
      applyFormState(fromConfig(result.config));
      configLoaded = true;
    } catch (e) {
      loadError = String(e);
    } finally {
      loading = false;
    }
  }

  function applyFormState(form: ForwarderConfigFormState): void {
    generalDisplayName = form.generalDisplayName;
    serverBaseUrl = form.serverBaseUrl;
    serverForwardersWsPath = form.serverForwardersWsPath;
    authTokenFile = form.authTokenFile;
    journalSqlitePath = form.journalSqlitePath;
    journalPruneWatermarkPct = form.journalPruneWatermarkPct;
    uplinkBatchMode = form.uplinkBatchMode;
    uplinkBatchFlushMs = form.uplinkBatchFlushMs;
    uplinkBatchMaxEvents = form.uplinkBatchMaxEvents;
    statusHttpBind = form.statusHttpBind;
    readers = form.readers.map((reader) => ({ ...reader }));
  }

  function currentFormState(): ForwarderConfigFormState {
    return {
      generalDisplayName,
      serverBaseUrl,
      serverForwardersWsPath,
      authTokenFile,
      journalSqlitePath,
      journalPruneWatermarkPct,
      uplinkBatchMode,
      uplinkBatchFlushMs,
      uplinkBatchMaxEvents,
      statusHttpBind,
      readers: readers.map((reader) => ({ ...reader })),
    };
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
          text: saveSuccessMessage(result.restart_needed),
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
    saveSection("general", toGeneralPayload(currentFormState()));
  }
  function saveServer() {
    saveSection("server", toServerPayload(currentFormState()));
  }
  function saveAuth() {
    saveSection("auth", toAuthPayload(currentFormState()));
  }
  function saveJournal() {
    saveSection("journal", toJournalPayload(currentFormState()));
  }
  function saveUplink() {
    saveSection("uplink", toUplinkPayload(currentFormState()));
  }
  function saveStatusHttp() {
    saveSection("status_http", toStatusHttpPayload(currentFormState()));
  }
  function saveReaders() {
    saveSection("readers", toReadersPayload(currentFormState()));
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
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <!-- General -->
        <Card title="General">
          <label class="block text-sm font-medium text-text-secondary mb-1">
            Display Name
            <input type="text" bind:value={generalDisplayName} class="mt-1 {inputClass}" />
          </label>
          <button
            class={saveBtnClass}
            onclick={saveGeneral}
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
            <label class="block text-sm font-medium text-text-secondary">
              Base URL *
              <input type="text" bind:value={serverBaseUrl} required class="mt-1 {inputClass}" />
            </label>
            <label class="block text-sm font-medium text-text-secondary">
              Forwarders WS Path
              <input type="text" bind:value={serverForwardersWsPath} class="mt-1 {inputClass}" />
            </label>
          </div>
          <button
            class={saveBtnClass}
            onclick={saveServer}
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
      </div>

      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <!-- Auth -->
        <Card title="Auth">
          <label class="block text-sm font-medium text-text-secondary mb-1">
            Token File Path *
            <input type="text" bind:value={authTokenFile} required class="mt-1 {inputClass}" />
          </label>
          <button
            class={saveBtnClass}
            onclick={saveAuth}
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
            <label class="block text-sm font-medium text-text-secondary">
              SQLite Path
              <input type="text" bind:value={journalSqlitePath} class="mt-1 {inputClass}" />
            </label>
            <label class="block text-sm font-medium text-text-secondary">
              Prune Watermark %
              <input type="number" bind:value={journalPruneWatermarkPct} min="0" max="100" class="mt-1 {inputClass}" />
            </label>
          </div>
          <button
            class={saveBtnClass}
            onclick={saveJournal}
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
      </div>

      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <!-- Uplink -->
        <Card title="Uplink">
          <div class="space-y-3">
            <label class="block text-sm font-medium text-text-secondary">
              Batch Mode
              <input type="text" bind:value={uplinkBatchMode} class="mt-1 {inputClass}" />
            </label>
            <label class="block text-sm font-medium text-text-secondary">
              Batch Flush (ms)
              <input type="number" bind:value={uplinkBatchFlushMs} min="0" class="mt-1 {inputClass}" />
            </label>
            <label class="block text-sm font-medium text-text-secondary">
              Batch Max Events
              <input type="number" bind:value={uplinkBatchMaxEvents} min="0" class="mt-1 {inputClass}" />
            </label>
          </div>
          <button
            class={saveBtnClass}
            onclick={saveUplink}
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
            <input type="text" bind:value={statusHttpBind} class="mt-1 {inputClass}" />
          </label>
          <button
            class={saveBtnClass}
            onclick={saveStatusHttp}
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
      </div>

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
                      aria-label="Reader {i + 1} target"
                      class={inputClass}
                    />
                  </td>
                  <td class="py-1.5 px-2">
                    <input
                      type="checkbox"
                      bind:checked={reader.enabled}
                      aria-label="Reader {i + 1} enabled"
                      class="accent-accent"
                    />
                  </td>
                  <td class="py-1.5 px-2">
                    <input
                      type="number"
                      bind:value={reader.local_fallback_port}
                      min="1"
                      max="65535"
                      aria-label="Reader {i + 1} fallback port"
                      class={inputClass}
                    />
                  </td>
                  <td class="py-1.5 px-2">
                    <button
                      onclick={() => removeReader(i)}
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
            onclick={addReader}
            class="px-3 py-1.5 text-xs font-medium rounded-md bg-surface-2 border border-border text-text-secondary cursor-pointer hover:bg-surface-3"
          >
            + Add Reader
          </button>
          <button
            class={saveBtnClass}
            onclick={saveReaders}
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
