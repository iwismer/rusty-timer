<script lang="ts">
  import { HelpTip } from "@rusty-timer/shared-ui";
  import {
    store,
    getConfigDirty,
    getConnectionState,
    getConnectionBadgeState,
    saveProfile,
    reconnectThinNode,
    saveDbfConfig,
    clearDbfFile,
    setEditThinNodeUrl,
    setEditToken,
    setEditReceiverId,
  } from "$lib/store.svelte";
  import { inputClass, btnPrimary, btnSecondary } from "$lib/ui-classes";

  const dotClass: Record<"ok" | "warn" | "err", string> = {
    ok: "bg-status-ok",
    warn: "bg-status-warn",
    err: "bg-status-err",
  };

  function getDbfDirty(): boolean {
    return (
      store.editDbfEnabled !== store.dbfEnabled ||
      store.editDbfPath !== store.dbfPath
    );
  }

  function approvalLabel(): string | null {
    const thin = store.status?.thin_node;
    if (!thin?.configured) return null;
    if (thin.waiting_for_approval)
      return thin.message ?? "Waiting for thin-node approval";
    if (thin.reachable === false)
      return thin.message ?? "Thin node unreachable";
    if (thin.approval_state === "active") return "Thin node approved";
    return thin.message;
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
      Thin-node URL
      <HelpTip
        fieldKey="thin_node_url"
        sectionKey="config"
        context="receiver"
      />
      <input
        data-testid="thin-node-url-input"
        class="{inputClass} mt-1"
        value={store.editThinNodeUrl}
        oninput={(e) => setEditThinNodeUrl(e.currentTarget.value)}
        placeholder="https://thin-node.example.com"
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
        <p class="mt-1 text-xs text-text-muted">
          Connects automatically to the forwarder over the peer-to-peer link.
        </p>
        {#if approvalLabel()}
          <p
            data-testid="thin-node-approval-state"
            class="mt-2 text-xs {store.status?.thin_node?.waiting_for_approval
              ? 'text-status-warn'
              : store.status?.thin_node?.reachable === false
                ? 'text-status-err'
                : 'text-text-muted'}"
          >
            {approvalLabel()}
          </p>
        {/if}
      </div>

      <div class="flex shrink-0 items-center gap-2">
        <span
          data-testid="config-connection-state"
          class="flex items-center gap-2 text-sm text-text-primary"
        >
          <span
            class="h-2 w-2 rounded-full {dotClass[getConnectionBadgeState()]}"
          ></span>
          {connectionLabel(getConnectionState())}
        </span>
        <button
          data-testid="reconnect-thin-node-btn"
          class="px-2 py-1 text-xs rounded-md bg-surface-0 text-text-secondary border border-border cursor-pointer hover:bg-surface-2 disabled:opacity-50"
          onclick={() => void reconnectThinNode()}
          disabled={getConfigDirty() || store.saving}
          title={getConfigDirty()
            ? "Save thin-node configuration before reconnecting"
            : "Retry thin-node discovery and P2P subscriptions"}
        >
          Reconnect
        </button>
      </div>
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
