<script lang="ts">
  import type { Snippet } from "svelte";
  import { setContext } from "svelte";
  import { resolveHeaderBgClass } from "../lib/card-logic";
  import type { HelpContextName } from "../lib/help/help-types";
  import HelpDialog from "./HelpDialog.svelte";
  import { HELP_OPEN_MODAL_KEY } from "./HelpTip.svelte";

  let {
    title = undefined,
    headerBg = false,
    borderStatus = undefined,
    helpSection = undefined,
    helpContext = undefined,
    header,
    children,
  }: {
    title?: string;
    headerBg?: boolean;
    /** Set to "ok", "warn", or "err" to show a colored border */
    borderStatus?: "ok" | "warn" | "err";
    helpSection?: string;
    helpContext?: HelpContextName;
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

  let helpDialogOpen = $state(false);
  let helpScrollToField = $state<string | undefined>(undefined);

  function openHelp(fieldKey?: string) {
    helpScrollToField = fieldKey;
    helpDialogOpen = true;
  }

  function closeHelp() {
    helpDialogOpen = false;
    helpScrollToField = undefined;
  }

  // Only provide help context when this Card is configured for help.
  // `setContext` runs during component initialization, so the initial prop
  // value is the intended one here.
  // svelte-ignore state_referenced_locally
  if (helpSection) {
    setContext(HELP_OPEN_MODAL_KEY, (fieldKey?: string) => {
      openHelp(fieldKey);
    });
  }
</script>

<section class="overflow-hidden rounded-lg bg-surface-1 border {borderClass}">
  {#if title || header || helpSection}
    <div
      class="px-4 py-3 border-b border-border flex flex-wrap items-center gap-3 rounded-t-lg {headerBgClass}"
    >
      {#if header}
        {@render header()}
      {:else}
        <h2 class="text-sm font-semibold text-text-primary">{title}</h2>
      {/if}
      {#if helpSection && helpContext}
        <button
          onclick={() => openHelp()}
          class="ml-auto inline-flex items-center justify-center w-5 h-5 rounded-full border border-border text-text-muted hover:text-accent hover:border-accent text-xs font-bold cursor-pointer bg-transparent transition-colors"
          aria-label="Help for {title ?? helpSection}"
          type="button"
        >?</button>
      {/if}
    </div>
  {/if}
  <div class="p-4">
    {#if children}
      {@render children()}
    {/if}
  </div>
</section>

{#if helpSection && helpContext}
  <HelpDialog
    open={helpDialogOpen}
    sectionKey={helpSection}
    context={helpContext}
    scrollToField={helpScrollToField}
    onClose={closeHelp}
  />
{/if}
