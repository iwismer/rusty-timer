<script lang="ts">
  import { getContext, hasContext, onDestroy } from "svelte";
  import { getField } from "../lib/help/index";
  import type { HelpContextName } from "../lib/help/help-types";
  import { computePopoverStyle } from "../lib/help-tip";

  export const HELP_OPEN_MODAL_KEY = "help-open-modal";

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

  // Injected by parent Card component when helpSection is set. Falls back to onOpenModal prop.
  const contextOpenHelp = hasContext(HELP_OPEN_MODAL_KEY)
    ? getContext<(fieldKey?: string) => void>(HELP_OPEN_MODAL_KEY)
    : undefined;

  let field = $derived(getField(context, sectionKey, fieldKey));

  $effect(() => {
    if (!field) {
      console.warn(
        `[HelpTip] No help found for field="${fieldKey}" section="${sectionKey}" context="${context}". Check for typos.`,
      );
    }
  });

  let btnEl: HTMLButtonElement | undefined = $state();
  let showingPopover = $state(false);
  let popoverStyle = $state("");
  let showTimer: ReturnType<typeof setTimeout> | undefined;
  let hideTimer: ReturnType<typeof setTimeout> | undefined;

  function updatePosition() {
    if (btnEl) {
      const rect = btnEl.getBoundingClientRect();
      popoverStyle = computePopoverStyle(rect, window.innerWidth, window.innerHeight);
    }
  }

  function scheduleShow() {
    clearTimeout(hideTimer);
    showTimer = setTimeout(() => {
      updatePosition();
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
    updatePosition();
    showingPopover = true;
  }

  function handleClick() {
    if (onOpenModal) {
      showingPopover = false;
      onOpenModal(fieldKey);
    } else if (contextOpenHelp) {
      showingPopover = false;
      contextOpenHelp(fieldKey);
    } else {
      console.warn(
        `[HelpTip] No modal handler for field="${fieldKey}". Ensure HelpTip is inside a Card with helpSection, or pass onOpenModal.`,
      );
    }
  }

  onDestroy(() => {
    clearTimeout(showTimer);
    clearTimeout(hideTimer);
  });

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      handleClick();
    }
  }
</script>

{#if field}
  <span class="inline-flex items-center ml-1">
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
        class="fixed z-50 w-72 p-3 rounded-lg border border-border bg-surface-1 shadow-lg text-sm"
        style={popoverStyle}
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
          <p class="text-xs font-medium text-status-ok m-0">Recommended: {field.recommended}</p>
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
