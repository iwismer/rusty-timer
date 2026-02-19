<script lang="ts">
  export let variant: "ok" | "warn" | "err" = "warn";
  export let message: string;
  export let actionLabel: string | undefined = undefined;
  export let actionBusy: boolean = false;
  export let onAction: (() => void) | undefined = undefined;

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
</script>

<div
  class="rounded-md px-4 py-3 flex items-center justify-between text-sm border {styles[
    variant
  ]}"
>
  <span class="font-medium">{message}</span>
  {#if actionLabel && onAction}
    <button
      on:click={onAction}
      disabled={actionBusy}
      class="px-3 py-1 text-xs font-medium rounded-md text-white border-none cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed {btnStyles[
        variant
      ]}"
    >
      {actionBusy ? "Applying..." : actionLabel}
    </button>
  {/if}
</div>
