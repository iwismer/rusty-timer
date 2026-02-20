<script lang="ts">
  import type { Snippet } from "svelte";
  import { resolveHeaderBgClass } from "../lib/card-logic";

  let {
    title = undefined,
    headerBg = false,
    borderStatus = undefined,
    header,
    children,
  }: {
    title?: string;
    headerBg?: boolean;
    /** Set to "ok", "warn", or "err" to show a colored border */
    borderStatus?: "ok" | "warn" | "err";
    header?: Snippet;
    children?: Snippet;
  } = $props();

  const borderMap: Record<string, string> = {
    ok: "border-status-ok-border",
    warn: "border-status-warn-border",
    err: "border-status-err-border",
  };

  let borderClass = $derived(borderStatus ? borderMap[borderStatus] : "border-border");
  let headerBgClass = $derived(resolveHeaderBgClass(borderStatus, headerBg));
</script>

<section class="rounded-lg overflow-hidden bg-surface-1 border {borderClass}">
  {#if title || header}
    <div
      class="px-4 py-3 border-b border-border flex items-center gap-3 {headerBgClass}"
    >
      {#if header}
        {@render header()}
      {:else}
        <h2 class="text-sm font-semibold text-text-primary">{title}</h2>
      {/if}
    </div>
  {/if}
  <div class="p-4">
    {#if children}
      {@render children()}
    {/if}
  </div>
</section>
