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
  import ReceiverModeConfig from "$lib/components/ReceiverModeConfig.svelte";

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
      {store.saving ? "Saving\u2026" : "Save"}
    </button>
  </div>

  <ReceiverModeConfig />

  <section class="mt-6 rounded-lg border border-border bg-surface-1 p-4">
    <p class="text-xs font-medium text-text-muted mb-3">Race Director</p>

    <div>
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
</div>
