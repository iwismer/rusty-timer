<script lang="ts">
  import { HelpTip } from "@rusty-timer/shared-ui";
  import {
    store,
    getConfigDirty,
    saveProfile,
    setEditServerUrl,
    setEditToken,
    setEditReceiverId,
  } from "$lib/store.svelte";

  const inputClass =
    "w-full px-3 py-1.5 text-sm rounded-md bg-surface-0 border border-border text-text-primary font-mono focus:outline-none focus:ring-1 focus:ring-accent";
  const btnPrimary =
    "px-3 py-1.5 text-sm font-medium rounded-md text-white bg-accent border-none cursor-pointer hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed";
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
</div>
