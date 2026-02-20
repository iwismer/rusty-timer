<script lang="ts">
  let {
    version,
    busy = false,
    onApply,
  }: {
    version: string;
    busy?: boolean;
    onApply: () => void;
  } = $props();
</script>

<div
  data-testid="update-banner"
  class="rounded-md px-4 py-3 flex items-center justify-between text-sm border bg-status-ok-bg border-status-ok-border text-status-ok"
>
  <span class="font-medium">Update v{version} available</span>
  <button
    data-testid="apply-update-btn"
    onclick={onApply}
    disabled={busy}
    class="px-3 py-1 text-xs font-medium rounded-md text-white border-none cursor-pointer bg-status-ok disabled:opacity-50 disabled:cursor-not-allowed"
  >
    {busy ? "Applying..." : "Update Now"}
  </button>
</div>
