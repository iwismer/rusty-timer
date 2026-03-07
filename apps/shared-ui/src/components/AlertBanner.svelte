<script lang="ts">
  let {
    variant = "warn",
    message,
    actionLabel = undefined,
    actionBusy = false,
    onAction = undefined,
    onDismiss = undefined,
  }: {
    variant?: "ok" | "warn" | "err";
    message: string;
    actionLabel?: string;
    actionBusy?: boolean;
    onAction?: () => void;
    onDismiss?: () => void;
  } = $props();

  const styles = {
    ok: "bg-status-ok-bg border-status-ok-border text-status-ok",
    warn: "bg-status-warn-bg border-status-warn-border text-status-warn",
    err: "bg-status-err-bg border-status-err-border text-status-err",
  };

  const btnStyles = {
    ok: "bg-status-ok",
    warn: "bg-status-warn",
    err: "bg-status-err",
  };

  const dismissStyles = {
    ok: "text-status-ok",
    warn: "text-status-warn",
    err: "text-status-err",
  };
</script>

<div
  class="rounded-md px-4 py-3 flex items-center justify-between text-sm border {styles[
    variant
  ]}"
>
  <span class="font-medium">{message}</span>
  <div class="flex items-center gap-2">
    {#if actionLabel && onAction}
      <button
        onclick={onAction}
        disabled={actionBusy}
        class="px-3 py-1 text-xs font-medium rounded-md text-white border-none cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed {btnStyles[
          variant
        ]}"
      >
        {actionBusy ? "Applying..." : actionLabel}
      </button>
    {/if}
    {#if onDismiss}
      <button
        onclick={onDismiss}
        class="ml-1 px-1 text-sm font-medium leading-none opacity-70 hover:opacity-100 cursor-pointer bg-transparent border-none {dismissStyles[
          variant
        ]}"
        aria-label="Dismiss"
      >
        ✕
      </button>
    {/if}
  </div>
</div>
