<script lang="ts">
  let {
    version,
    status,
    busy = false,
    onDownload,
    onApply,
  }: {
    version: string;
    status: "available" | "downloaded";
    busy?: boolean;
    onDownload: () => void;
    onApply: () => void;
  } = $props();

  let isDownloaded = $derived(status === "downloaded");
</script>

<div
  data-testid="update-banner"
  class="rounded-md px-4 py-3 flex items-center justify-between text-sm border bg-status-ok-bg border-status-ok-border text-status-ok"
>
  <span class="font-medium">
    {isDownloaded ? `Update v${version} ready to install` : `Update v${version} available`}
  </span>
  <button
    data-testid={isDownloaded ? "apply-update-btn" : "download-update-btn"}
    onclick={isDownloaded ? onApply : onDownload}
    disabled={busy}
    class="px-3 py-1 text-xs font-medium rounded-md text-white border-none cursor-pointer bg-status-ok disabled:opacity-50 disabled:cursor-not-allowed"
  >
    {#if isDownloaded}
      {busy ? "Applying..." : "Update Now"}
    {:else}
      {busy ? "Downloading..." : "Download"}
    {/if}
  </button>
</div>
