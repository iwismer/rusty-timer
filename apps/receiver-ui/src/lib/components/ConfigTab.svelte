<script lang="ts">
  import { HelpTip } from "@rusty-timer/shared-ui";
  import {
    store,
    getConfigDirty,
    getConnectionState,
    handleConnect,
    handleDisconnect,
    saveProfile,
    saveDbfConfig,
    clearDbfFile,
    setEditServerUrl,
    setEditToken,
    setEditReceiverId,
  } from "$lib/store.svelte";
  import {
    inputClass,
    btnPrimary,
    btnSecondary,
    btnDisconnect,
  } from "$lib/ui-classes";

  function getDbfDirty(): boolean {
    return (
      store.editDbfEnabled !== store.dbfEnabled ||
      store.editDbfPath !== store.dbfPath
    );
  }

  function connectionLabel(state: string): string {
    switch (state) {
      case "connected":
        return "Connected";
      case "disconnected":
        return "Disconnected";
      case "connecting":
        return "Connecting...";
      case "disconnecting":
        return "Disconnecting...";
      default:
        return "Unknown";
    }
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
        placeholder="wss://server:8080"
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
      />
    </label>
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
        <p
          data-testid="config-connection-state"
          class="mt-1 text-sm text-text-primary"
        >
          {connectionLabel(getConnectionState())}
        </p>
      </div>

      {#if getConnectionState() === "connected"}
        <button
          data-testid="config-connect-toggle-btn"
          class={btnDisconnect}
          onclick={() => handleDisconnect()}
          disabled={store.connectBusy}
        >
          Disconnect
        </button>
      {:else if getConnectionState() === "disconnected"}
        <button
          data-testid="config-connect-toggle-btn"
          class={btnPrimary}
          onclick={() => handleConnect()}
          disabled={store.connectBusy || !store.savedServerUrl}
        >
          Connect
        </button>
      {:else}
        <button
          data-testid="config-connect-toggle-btn"
          class={btnPrimary}
          disabled
        >
          {getConnectionState() === "disconnecting"
            ? "Disconnecting..."
            : "Connecting..."}
        </button>
      {/if}
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
