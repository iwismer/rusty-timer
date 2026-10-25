<script lang="ts">
  import { ForwarderWifiManager } from "@rusty-timer/shared-ui";
  import { createForwarderWifiApi } from "$lib/api";
  import { btnSecondary } from "$lib/ui-classes";

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

  function handleKeydown(e: KeyboardEvent) {
    if (open && e.key === "Escape") {
      onClose();
    }
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      onClose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open && endpointId}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    class="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/60 px-4 py-8"
    role="dialog"
    tabindex="-1"
    aria-modal="true"
    aria-label="Forwarder Wi-Fi management"
    data-testid="forwarder-wifi-modal"
    onclick={handleBackdropClick}
  >
    <div
      class="w-full max-w-3xl rounded-lg border border-border bg-surface-0 shadow-xl"
    >
      <div
        class="flex items-start justify-between gap-4 border-b border-border px-5 py-4"
      >
        <div>
          <h2 class="m-0 text-lg font-semibold text-text-primary">
            Wi-Fi: {displayName ?? endpointId}
          </h2>
          <p class="mt-1 font-mono text-xs text-text-muted">{endpointId}</p>
        </div>
        <button class={btnSecondary} type="button" onclick={onClose}>
          Close
        </button>
      </div>

      <div class="px-5 py-4">
        {#key endpointId}
          <ForwarderWifiManager
            wifiApi={createForwarderWifiApi(endpointId)}
            isRemoteP2p={true}
          />
        {/key}
      </div>
    </div>
  </div>
{/if}
