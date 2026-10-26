<script lang="ts">
  import { HelpTip } from "@rusty-timer/shared-ui";
  import {
    store,
    getConfigDirty,
    saveProfile,
    saveDbfConfig,
    clearDbfFile,
    saveRdImportConfig,
    setEditServerUrl,
    setEditToken,
    setEditReceiverId,
    openHelp,
  } from "$lib/store.svelte";
  import { inputClass, btnPrimary, btnSecondary } from "$lib/ui-classes";
  import ForwarderConfigModal from "./ForwarderConfigModal.svelte";
  import ForwarderWifiModal from "./ForwarderWifiModal.svelte";

  let selectedConfigEndpointId: string | null = null;
  let selectedWifiEndpointId: string | null = null;

  $: selectedConfigForwarder =
    store.connections?.forwarders.find(
      (f) => f.endpoint_id === selectedConfigEndpointId,
    ) ?? null;

  $: selectedWifiForwarder =
    store.connections?.forwarders.find(
      (f) => f.endpoint_id === selectedWifiEndpointId,
    ) ?? null;

  function getDbfDirty(): boolean {
    return (
      store.editDbfEnabled !== store.dbfEnabled ||
      store.editDbfFlushIntervalMs !== store.dbfFlushIntervalMs
    );
  }

  function getRdImportDirty(): boolean {
    return (
      store.editRdImportEnabled !== store.rdImportEnabled ||
      store.editRdImportDir !== store.rdImportDir ||
      store.editRdImportIntervalSecs !== store.rdImportIntervalSecs
    );
  }
</script>

<div class="max-w-[500px] mx-auto px-6 py-6">
  <div class="grid gap-4">
    <label class="block text-xs font-medium text-text-muted">
      Receiver ID
      <HelpTip
        fieldKey="receiver_id"
        sectionKey="config"
        context="receiver"
        onOpenModal={openHelp}
      />
      <input
        data-testid="receiver-id-input"
        class="{inputClass} mt-1"
        value={store.editReceiverId}
        oninput={(e) => setEditReceiverId(e.currentTarget.value)}
        placeholder="recv-a1b2c3d4"
      />
    </label>

    <label class="block text-xs font-medium text-text-muted">
      Server URL
      <HelpTip
        fieldKey="server_url"
        sectionKey="config"
        context="receiver"
        onOpenModal={openHelp}
      />
      <input
        data-testid="server-url-input"
        class="{inputClass} mt-1"
        value={store.editServerUrl}
        oninput={(e) => setEditServerUrl(e.currentTarget.value)}
        placeholder="https://server.example.com"
        disabled={store.serverSource === "env"}
      />
    </label>

    <label class="block text-xs font-medium text-text-muted">
      Token
      <HelpTip
        fieldKey="token"
        sectionKey="config"
        context="receiver"
        onOpenModal={openHelp}
      />
      <input
        data-testid="token-input"
        type="password"
        class="{inputClass} mt-1"
        value={store.editToken}
        oninput={(e) => setEditToken(e.currentTarget.value)}
        placeholder="auth token"
        disabled={store.serverSource === "env"}
      />
    </label>

    {#if store.serverSource === "env"}
      <p data-testid="server-env-override-note" class="text-xs text-text-muted">
        Server URL and token are set by environment variables (<code
          >RT_P2P_SERVER_URL</code
        >
        / <code>RT_P2P_SERVER_TOKEN</code>) and override the stored profile.
        Unset them to edit here.
      </p>
    {/if}
  </div>

  <div class="mt-4">
    <button
      data-testid="save-config-btn"
      class={btnPrimary}
      onclick={() => saveProfile()}
      disabled={!getConfigDirty() || store.saving}
    >
      {store.saving ? "Saving…" : "Save"}
    </button>
  </div>

  <section class="mt-6 rounded-lg border border-border bg-surface-1 p-4">
    <p class="text-xs font-medium text-text-muted mb-3">Race Director</p>

    <label class="block text-xs font-medium text-text-muted">
      Folder
      <HelpTip
        fieldKey="rd_import_dir"
        sectionKey="rd_import"
        context="receiver"
        onOpenModal={openHelp}
      />
      <input
        data-testid="rd-import-dir-input"
        class="{inputClass} mt-1"
        value={store.editRdImportDir}
        oninput={(e) => (store.editRdImportDir = e.currentTarget.value)}
        placeholder="C:\Winrace\Files"
      />
    </label>

    <div class="mt-5 border-t border-border pt-4">
      <p class="text-xs font-medium text-text-muted mb-3">
        Pull participant/chip data from Race Director
      </p>

      <label
        class="flex items-center gap-2 text-xs text-text-primary cursor-pointer"
      >
        <input
          data-testid="rd-import-enabled-toggle"
          type="checkbox"
          checked={store.editRdImportEnabled}
          onchange={(e) =>
            (store.editRdImportEnabled = e.currentTarget.checked)}
          class="accent-accent"
        />
        Poll Race Director DBF files for participant and chip data
        <HelpTip
          fieldKey="rd_import_enabled"
          sectionKey="rd_import"
          context="receiver"
          onOpenModal={openHelp}
        />
      </label>

      <label class="block text-xs font-medium text-text-muted mt-3">
        Poll interval (seconds)
        <HelpTip
          fieldKey="rd_import_interval"
          sectionKey="rd_import"
          context="receiver"
          onOpenModal={openHelp}
        />
        <input
          data-testid="rd-import-interval-input"
          type="number"
          min="1"
          step="1"
          class="{inputClass} mt-1"
          value={store.editRdImportIntervalSecs}
          oninput={(e) =>
            (store.editRdImportIntervalSecs = Number(e.currentTarget.value))}
        />
      </label>

      <div class="mt-3 flex items-center gap-2">
        <button
          data-testid="save-rd-import-btn"
          class={btnPrimary}
          onclick={() => saveRdImportConfig()}
          disabled={!getRdImportDirty() || store.rdImportSaving}
        >
          {store.rdImportSaving ? "Saving\u2026" : "Save Import Config"}
        </button>
      </div>
    </div>

    <div class="mt-5 border-t border-border pt-4">
      <p class="text-xs font-medium text-text-muted mb-3">
        Send reads to Race Director
      </p>

      <label
        class="flex items-center gap-2 text-xs text-text-primary cursor-pointer"
      >
        <input
          data-testid="dbf-enabled-toggle"
          type="checkbox"
          checked={store.editDbfEnabled}
          onchange={(e) => (store.editDbfEnabled = e.currentTarget.checked)}
          class="accent-accent"
        />
        Write reads to Ipico Direct file for Race Director
        <HelpTip
          fieldKey="dbf_enabled"
          sectionKey="dbf_output"
          context="receiver"
          onOpenModal={openHelp}
        />
      </label>

      <p class="mt-3 text-xs text-text-muted">
        Writes to <code>{store.editRdImportDir}\IPICO.DBF</code>.
      </p>

      {#if store.editDbfEnabled}
        <label class="mt-3 flex items-center gap-2 text-xs text-text-primary">
          DBF write interval (seconds)
          <HelpTip
            fieldKey="dbf_flush_interval"
            sectionKey="dbf_output"
            context="receiver"
            onOpenModal={openHelp}
          />
          <input
            data-testid="dbf-flush-interval-input"
            type="number"
            min="0.25"
            max="5"
            step="0.25"
            value={store.editDbfFlushIntervalMs / 1000}
            oninput={(e) => {
              const seconds = Number(e.currentTarget.value);
              if (Number.isFinite(seconds) && seconds > 0) {
                store.editDbfFlushIntervalMs = Math.round(seconds * 1000);
              }
            }}
            class="w-20 rounded border border-border bg-surface px-2 py-1 text-xs"
          />
        </label>
        <p class="mt-1 text-xs text-text-muted">
          How often new reads are written to the file (0.25–5 seconds).
        </p>
      {/if}

      <div class="mt-3 flex items-center gap-2">
        <button
          data-testid="save-dbf-btn"
          class={btnPrimary}
          onclick={() => saveDbfConfig()}
          disabled={!getDbfDirty() || store.dbfSaving}
        >
          {store.dbfSaving ? "Saving\u2026" : "Save DBF Config"}
        </button>
        <button
          data-testid="clear-dbf-btn"
          class={btnSecondary}
          onclick={() => clearDbfFile()}
          disabled={store.dbfClearing}
        >
          {store.dbfClearing ? "Clearing\u2026" : "Clear DBF File"}
        </button>
        <HelpTip
          fieldKey="clear_dbf"
          sectionKey="dbf_output"
          context="receiver"
          onOpenModal={openHelp}
        />
      </div>
    </div>
  </section>

  <section class="mt-6 rounded-lg border border-border bg-surface-1 p-4">
    <div class="flex items-center justify-between mb-3">
      <p class="text-xs font-medium text-text-muted">
        Timing Boxes &amp; Forwarders
      </p>
      <HelpTip
        fieldKey="forwarder_state"
        sectionKey="connections"
        context="receiver"
        onOpenModal={openHelp}
      />
    </div>

    {#if store.connections?.forwarders && store.connections.forwarders.length > 0}
      <div class="space-y-3">
        {#each store.connections.forwarders as fwd}
          <div
            class="p-3 rounded-lg border border-border bg-surface-0 flex flex-col sm:flex-row sm:items-center justify-between gap-3"
          >
            <div>
              <div class="flex items-center gap-2">
                <span class="font-mono font-semibold text-xs text-text-primary">
                  {fwd.display_name || fwd.endpoint_id}
                </span>
                <span
                  class="text-[10px] font-semibold uppercase px-1.5 py-0.5 rounded {fwd.state ===
                  'subscribed'
                    ? 'text-status-ok bg-status-ok-bg border border-status-ok-border'
                    : 'text-text-muted bg-surface-2 border border-border'}"
                >
                  {fwd.state}
                </span>
              </div>
              <div class="text-[11px] text-text-muted font-mono mt-0.5">
                {fwd.endpoint_id} &middot; {fwd.readers?.length ?? 0} reader target(s)
              </div>
            </div>

            <div class="flex items-center gap-2 shrink-0">
              <button
                type="button"
                data-testid={`forwarder-config-btn-${fwd.endpoint_id}`}
                class={btnSecondary}
                class:!px-2.5={true}
                class:!py-1={true}
                class:!text-xs={true}
                onclick={() => (selectedConfigEndpointId = fwd.endpoint_id)}
              >
                Configure Box
              </button>
              {#if fwd.wifi_control_available}
                <button
                  type="button"
                  data-testid={`forwarder-wifi-btn-${fwd.endpoint_id}`}
                  class={btnSecondary}
                  class:!px-2.5={true}
                  class:!py-1={true}
                  class:!text-xs={true}
                  onclick={() => (selectedWifiEndpointId = fwd.endpoint_id)}
                >
                  Wi-Fi Settings
                </button>
              {/if}
            </div>
          </div>
        {/each}
      </div>
    {:else}
      <p class="text-xs text-text-muted italic py-1">
        No timing boxes or forwarders currently detected.
      </p>
    {/if}
  </section>

  <ForwarderConfigModal
    open={selectedConfigEndpointId !== null}
    endpointId={selectedConfigEndpointId}
    displayName={selectedConfigForwarder?.display_name ?? null}
    onClose={() => (selectedConfigEndpointId = null)}
  />

  <ForwarderWifiModal
    open={selectedWifiEndpointId !== null}
    endpointId={selectedWifiEndpointId}
    displayName={selectedWifiForwarder?.display_name ?? null}
    onClose={() => (selectedWifiEndpointId = null)}
  />
</div>
