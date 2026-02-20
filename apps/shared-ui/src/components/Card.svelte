<script lang="ts">
  import { resolveHeaderBgClass } from "../lib/card-logic";

  export let title: string | undefined = undefined;
  export let headerBg: boolean = false;
  /** Set to "ok", "warn", or "err" to show a colored border */
  export let borderStatus: "ok" | "warn" | "err" | undefined = undefined;

  const borderMap: Record<string, string> = {
    ok: "border-status-ok-border",
    warn: "border-status-warn-border",
    err: "border-status-err-border",
  };

  $: borderClass = borderStatus ? borderMap[borderStatus] : "border-border";
  $: headerBgClass = resolveHeaderBgClass(borderStatus, headerBg);
</script>

<section class="rounded-lg overflow-hidden bg-surface-1 border {borderClass}">
  {#if title || $$slots.header}
    <div
      class="px-4 py-3 border-b border-border flex items-center gap-3 {headerBgClass}"
    >
      {#if $$slots.header}
        <slot name="header" />
      {:else}
        <h2 class="text-sm font-semibold text-text-primary">{title}</h2>
      {/if}
    </div>
  {/if}
  <div class="p-4">
    <slot />
  </div>
</section>
