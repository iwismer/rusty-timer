<script lang="ts">
  import {
    store,
    getConnectionState,
    handleConnect,
    handleDisconnect,
  } from "$lib/store.svelte";

  const btnPrimary =
    "px-3 py-1.5 text-sm font-medium rounded-md text-white bg-accent border-none cursor-pointer hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed";
  const btnDisconnect =
    "px-3 py-1.5 text-sm font-medium rounded-md text-status-err border border-status-err-border bg-status-err-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed";

  function dotColor(state: string): string {
    switch (state) {
      case "connected":
        return "bg-status-ok";
      case "disconnected":
        return "bg-status-err";
      case "connecting":
      case "disconnecting":
        return "bg-status-warn";
      default:
        return "bg-surface-2";
    }
  }

  function label(state: string): string {
    switch (state) {
      case "connected":
        return "Connected";
      case "disconnected":
        return "Disconnected";
      case "connecting":
        return "Connecting\u2026";
      case "disconnecting":
        return "Disconnecting\u2026";
      default:
        return "Unknown";
    }
  }
</script>

<div
  class="flex items-center justify-between px-3 h-9 bg-surface-1 border-b border-border shrink-0"
>
  <div class="flex items-center gap-2 min-w-0">
    <span
      class="w-2.5 h-2.5 rounded-full shrink-0 {dotColor(getConnectionState())}"
    ></span>
    <span class="text-sm text-text-primary shrink-0"
      >{label(getConnectionState())}</span
    >
    {#if store.savedServerUrl && getConnectionState() !== "disconnected"}
      <span
        class="text-xs text-text-muted truncate"
        title={store.savedServerUrl}>{store.savedServerUrl}</span
      >
    {/if}
  </div>

  <div class="py-1">
    {#if getConnectionState() === "connected"}
      <button
        data-testid="connect-toggle-btn"
        class={btnDisconnect}
        onclick={() => handleDisconnect()}
        disabled={store.connectBusy}
      >
        Disconnect
      </button>
    {:else if getConnectionState() === "disconnected"}
      <button
        data-testid="connect-toggle-btn"
        class={btnPrimary}
        onclick={() => handleConnect()}
        disabled={store.connectBusy || !store.savedServerUrl}
        title={!store.savedServerUrl ? "Configure server URL first" : ""}
      >
        Connect
      </button>
    {:else}
      <button data-testid="connect-toggle-btn" class={btnPrimary} disabled>
        {getConnectionState() === "disconnecting"
          ? "Disconnecting\u2026"
          : "Connecting\u2026"}
      </button>
    {/if}
  </div>
</div>
