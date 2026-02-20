<script lang="ts">
  let {
    open = false,
    title = "",
    message = "",
    confirmLabel = "Confirm",
    variant = "err",
    busy = false,
    onConfirm,
    onCancel,
  }: {
    open: boolean;
    title: string;
    message: string;
    confirmLabel?: string;
    variant?: "warn" | "err";
    busy?: boolean;
    onConfirm: () => void;
    onCancel: () => void;
  } = $props();

  let dialogEl: HTMLDialogElement | undefined = $state();

  $effect(() => {
    if (!dialogEl) return;
    if (open && !dialogEl.open) {
      dialogEl.showModal();
    } else if (!open && dialogEl.open) {
      dialogEl.close();
    }
  });

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    }
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === dialogEl) {
      onCancel();
    }
  }

  let confirmBg = $derived(
    variant === "warn"
      ? "bg-status-warn text-white"
      : "bg-status-err text-white",
  );
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<dialog
  bind:this={dialogEl}
  onkeydown={handleKeydown}
  onclick={handleBackdropClick}
  class="fixed inset-0 m-auto max-w-md w-full rounded-lg border border-border bg-surface-1 p-0 shadow-lg backdrop:bg-black/50"
>
  <div class="p-6">
    <h2 class="text-lg font-bold text-text-primary m-0 mb-2">{title}</h2>
    <p class="text-sm text-text-secondary m-0 mb-6">{message}</p>
    <div class="flex justify-end gap-3">
      <button
        onclick={onCancel}
        disabled={busy}
        class="px-4 py-2 text-sm font-medium rounded-md bg-surface-2 text-text-secondary border border-border cursor-pointer hover:bg-surface-3 disabled:opacity-50 disabled:cursor-not-allowed"
      >
        Cancel
      </button>
      <button
        onclick={onConfirm}
        disabled={busy}
        class="px-4 py-2 text-sm font-medium rounded-md {confirmBg} border-none cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed"
      >
        {busy ? "Working\u2026" : confirmLabel}
      </button>
    </div>
  </div>
</dialog>
