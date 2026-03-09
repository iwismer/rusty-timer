<script lang="ts">
  import { getContext } from "svelte";
  import { getField } from "../lib/help/index";
  import type { HelpContextName } from "../lib/help/help-types";
  import { resolvePopoverPosition } from "../lib/help-tip";

  let {
    fieldKey,
    sectionKey,
    context = "forwarder" as HelpContextName,
    onOpenModal = undefined as ((fieldKey: string) => void) | undefined,
  }: {
    fieldKey: string;
    sectionKey: string;
    context: HelpContextName;
    onOpenModal?: (fieldKey: string) => void;
  } = $props();

  const contextOpenHelp = getContext<((fieldKey?: string) => void) | undefined>(
    "help-open-modal",
  );

  let field = $derived(getField(context, sectionKey, fieldKey));
  let btnEl: HTMLButtonElement | undefined = $state();
  let showingPopover = $state(false);
  let popoverPosition = $state<"above" | "below">("below");
  let showTimer: ReturnType<typeof setTimeout> | undefined;
  let hideTimer: ReturnType<typeof setTimeout> | undefined;

  let positionClass = $derived(
    popoverPosition === "above"
      ? "bottom-full mb-2 left-0"
      : "top-full mt-2 left-0",
  );

  function scheduleShow() {
    clearTimeout(hideTimer);
    showTimer = setTimeout(() => {
      if (btnEl) {
        const rect = btnEl.getBoundingClientRect();
        popoverPosition = resolvePopoverPosition(rect, window.innerHeight);
      }
      showingPopover = true;
    }, 200);
  }

  function scheduleHide() {
    clearTimeout(showTimer);
    hideTimer = setTimeout(() => {
      showingPopover = false;
    }, 150);
  }

  function cancelHide() {
    clearTimeout(hideTimer);
  }

  function showPopover() {
    clearTimeout(hideTimer);
    if (btnEl) {
      const rect = btnEl.getBoundingClientRect();
      popoverPosition = resolvePopoverPosition(rect, window.innerHeight);
    }
    showingPopover = true;
  }

  function handleClick() {
    showingPopover = false;
    if (onOpenModal) {
      onOpenModal(fieldKey);
    } else if (contextOpenHelp) {
      contextOpenHelp(fieldKey);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      handleClick();
    }
  }
</script>

{#if field}
  <span class="relative inline-flex items-center ml-1">
    <button
      bind:this={btnEl}
      onmouseenter={scheduleShow}
      onmouseleave={scheduleHide}
      onfocus={showPopover}
      onblur={scheduleHide}
      onclick={handleClick}
      onkeydown={handleKeydown}
      class="inline-flex items-center justify-center w-4 h-4 rounded-full border border-border text-text-muted hover:text-accent hover:border-accent focus:text-accent focus:border-accent text-[10px] font-bold cursor-pointer bg-transparent transition-colors"
      aria-label="Help for {field.label}"
      type="button"
    >?</button>

    {#if showingPopover}
      <div
        class="absolute z-50 w-72 p-3 rounded-lg border border-border bg-surface-1 shadow-lg text-sm {positionClass}"
        role="tooltip"
        onmouseenter={cancelHide}
        onmouseleave={scheduleHide}
      >
        <p class="text-text-primary mb-1 m-0">{field.summary}</p>
        {#if field.default}
          <p class="text-xs text-text-muted m-0">
            Default: <code class="bg-surface-2 px-1 rounded">{field.default}</code>
          </p>
        {/if}
        {#if field.range}
          <p class="text-xs text-text-muted m-0">Valid: {field.range}</p>
        {/if}
        {#if field.recommended}
          <p class="text-xs font-medium text-accent m-0">Recommended: {field.recommended}</p>
        {/if}
        {#if onOpenModal || contextOpenHelp}
          <button
            onclick={handleClick}
            class="mt-2 text-xs text-accent hover:underline cursor-pointer bg-transparent border-none p-0"
            type="button"
          >
            More details...
          </button>
        {/if}
      </div>
    {/if}
  </span>
{/if}
