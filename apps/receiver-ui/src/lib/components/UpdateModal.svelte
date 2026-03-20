<script lang="ts">
  import { tick } from "svelte";
  import {
    closeUpdateModal,
    confirmUpdateInstall,
    store,
  } from "$lib/store.svelte";

  let dialogRef: HTMLDivElement | undefined = $state(undefined);

  function close() {
    closeUpdateModal();
  }

  function primaryLabel(): string {
    if (store.updateState?.busy) return "Installing...";
    if (store.updateState?.status === "downloaded") return "Restart to update";
    return "Download and install";
  }

  $effect(() => {
    if (store.updateModalOpen && store.updateState) {
      void tick().then(() => dialogRef?.focus());
    }
  });
</script>

{#if store.updateModalOpen && store.updateState}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
    bind:this={dialogRef}
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    onclick={close}
    onkeydown={(event) => {
      if (event.key === "Escape") close();
    }}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="w-full max-w-[520px] rounded-lg border border-border bg-surface-0 shadow-xl"
      onclick={(event) => event.stopPropagation()}
      onkeydown={(event) => event.stopPropagation()}
    >
      <div class="border-b border-border px-4 py-3">
        <h2 class="m-0 text-sm font-semibold text-text-primary">
          Update available
        </h2>
        <p class="m-0 mt-1 text-xs text-text-muted">
          v{store.updateState.currentVersion} to v{store.updateState.version}
        </p>
      </div>

      <div class="space-y-3 px-4 py-4">
        <p class="m-0 text-sm text-text-secondary">
          Install the latest receiver desktop update from this machine.
        </p>

        {#if store.updateState.notes}
          <div class="space-y-1">
            <h3
              class="m-0 text-xs font-semibold uppercase tracking-[0.08em] text-text-muted"
            >
              Release Notes
            </h3>
            <div
              class="max-h-40 overflow-y-auto rounded-md border border-border bg-surface-1 px-3 py-2 text-sm text-text-secondary whitespace-pre-line"
            >
              {store.updateState.notes}
            </div>
          </div>
        {/if}

        {#if store.updateState.error}
          <p class="m-0 text-sm text-status-err">{store.updateState.error}</p>
        {/if}
      </div>

      <div
        class="flex items-center justify-end gap-2 border-t border-border px-4 py-3"
      >
        <button
          type="button"
          class="rounded-md border border-border bg-surface-1 px-3 py-1.5 text-sm text-text-primary cursor-pointer hover:bg-surface-2"
          onclick={close}
        >
          Close
        </button>
        <button
          type="button"
          class="rounded-md border-none bg-accent px-3 py-1.5 text-sm font-medium text-white cursor-pointer hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50"
          onclick={() => void confirmUpdateInstall()}
          disabled={store.updateState.busy}
        >
          {primaryLabel()}
        </button>
      </div>
    </div>
  </div>
{/if}
