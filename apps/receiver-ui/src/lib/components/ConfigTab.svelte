<script lang="ts">
  import { HelpTip } from "@rusty-timer/shared-ui";
  import {
    store,
    getConfigDirty,
    saveProfile,
    reconnectServer,
    saveDbfConfig,
    clearDbfFile,
    setEditServerUrl,
    setEditToken,
    setEditReceiverId,
  } from "$lib/store.svelte";
  import { inputClass, btnPrimary, btnSecondary } from "$lib/ui-classes";

  function getDbfDirty(): boolean {
    return (
      store.editDbfEnabled !== store.dbfEnabled ||
      store.editDbfPath !== store.dbfPath
    );
  }
</script>

<div class="max-w-[500px] mx-auto px-6 py-6">
  <div class="grid gap-4">
    <label class="block text-xs font-medium text-text-muted">
      Receiver ID
      <HelpTip fieldKey="receiver_id" sectionKey="config" context="receiver" />
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
      <HelpTip fieldKey="server_url" sectionKey="config" context="receiver" />
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
      <HelpTip fieldKey="token" sectionKey="config" context="receiver" />
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

  <section class="mt-6 rounded-lg border border-border bg-surface-1 p-4">
    <div class="flex items-center justify-between gap-4">
      <div>
        <p class="text-xs font-medium text-text-muted">Connection</p>
        <p class="mt-1 text-xs text-text-muted">
          Connection status is shown in the Connections tab.
        </p>
      </div>

      <button
        data-testid="reconnect-server-btn"
        class="px-2 py-1 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
        onclick={() => void reconnectServer()}
        disabled={getConfigDirty() || store.saving}
        title={getConfigDirty()
          ? "Save server configuration before reconnecting"
          : "Retry server discovery and P2P subscriptions"}
      >
        Reconnect
      </button>
    </div>
  </section>

  <section class="mt-6 rounded-lg border border-border bg-surface-1 p-4">
    <p class="text-xs font-medium text-text-muted mb-3">Race Director Output</p>

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
    </label>

    <label class="block text-xs font-medium text-text-muted mt-3">
      File path
      <input
        data-testid="dbf-path-input"
        class="{inputClass} mt-1"
        value={store.editDbfPath}
        oninput={(e) => (store.editDbfPath = e.currentTarget.value)}
        placeholder="C:\winrace\Files\IPICO.DBF"
      />
    </label>

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
    </div>
  </section>
</div>
