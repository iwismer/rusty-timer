<script lang="ts">
  import {
    shouldCancelOnBackdropClick,
    shouldCancelOnEscape,
  } from "../lib/confirm-dialog";
  import { filterSectionContent } from "../lib/help-dialog";
  import { getSection } from "../lib/help/index";
  import type { HelpContextName } from "../lib/help/help-types";

  let {
    open = false,
    sectionKey = "",
    context = "forwarder" as HelpContextName,
    scrollToField = undefined as string | undefined,
    onClose,
    onNavigate = undefined as ((sectionKey: string) => void) | undefined,
  }: {
    open: boolean;
    sectionKey: string;
    context: HelpContextName;
    scrollToField?: string;
    onClose: () => void;
    onNavigate?: (sectionKey: string) => void;
  } = $props();

  let dialogEl: HTMLDialogElement | undefined = $state();
  let searchQuery = $state("");

  let section = $derived(getSection(context, sectionKey));
  let filtered = $derived(
    section ? filterSectionContent(section, searchQuery) : { fields: [], tips: [] },
  );

  $effect(() => {
    if (!dialogEl) return;
    if (open && !dialogEl.open) {
      if (!section) {
        console.warn(
          `[HelpDialog] No help section found for key "${sectionKey}" in context "${context}".`,
        );
        onClose();
        return;
      }
      dialogEl.showModal();
      searchQuery = "";
    } else if (!open && dialogEl.open) {
      dialogEl.close();
    }
    return () => {
      if (dialogEl?.open) dialogEl.close();
    };
  });

  $effect(() => {
    if (open && scrollToField) {
      // Small delay to allow dialog content to render
      const timer = setTimeout(() => {
        document
          .getElementById(`help-${scrollToField}`)
          ?.scrollIntoView({ behavior: "smooth", block: "start" });
      }, 50);
      return () => clearTimeout(timer);
    }
  });

  $effect(() => {
    if (!dialogEl) return;
    const el = dialogEl;
    const handleClick = (e: MouseEvent) => {
      if (shouldCancelOnBackdropClick(e.target, el)) {
        onClose();
      }
    };
    el.addEventListener("click", handleClick);
    return () => el.removeEventListener("click", handleClick);
  });

  function handleKeydown(e: KeyboardEvent) {
    if (shouldCancelOnEscape(e.key)) {
      e.preventDefault();
      onClose();
    }
  }
</script>

<dialog
  bind:this={dialogEl}
  onkeydown={handleKeydown}
  class="fixed inset-0 m-auto max-w-2xl w-full max-h-[80vh] rounded-lg border border-border bg-surface-1 p-0 shadow-lg backdrop:bg-black/50 overflow-hidden"
>
  {#if section}
    <div class="sticky top-0 bg-surface-1 border-b border-border px-6 py-4 z-10">
      <div class="flex items-center justify-between">
        <h2 class="text-lg font-bold text-text-primary m-0">{section.title}</h2>
        <button
          onclick={onClose}
          class="text-text-muted hover:text-text-primary text-lg cursor-pointer bg-transparent border-none p-1"
          aria-label="Close help"
          type="button"
        >&times;</button>
      </div>
      <input
        type="text"
        placeholder="Search this section..."
        bind:value={searchQuery}
        class="mt-3 w-full px-3 py-1.5 text-sm rounded-md border border-border bg-surface-0 text-text-primary placeholder:text-text-muted focus:outline-none focus:border-accent"
      />
    </div>
    <div class="px-6 py-4 overflow-y-auto max-h-[70vh]">
      <p class="text-sm text-text-secondary mb-4">{section.overview}</p>

      {#each filtered.fields as [fieldKey, field]}
        <div id="help-{fieldKey}" class="mb-6 scroll-mt-32">
          <h3 class="text-sm font-semibold text-text-primary mb-1">{field.label}</h3>
          <p class="text-sm text-text-secondary mb-2">{@html field.detail}</p>
          {#if field.default}
            <p class="text-xs text-text-muted">
              Default: <code class="bg-surface-2 px-1 rounded">{field.default}</code>
            </p>
          {/if}
          {#if field.range}
            <p class="text-xs text-text-muted">Valid: {field.range}</p>
          {/if}
          {#if field.recommended}
            <p class="text-xs font-medium text-accent">Recommended: {field.recommended}</p>
          {/if}
        </div>
      {/each}

      {#if filtered.fields.length === 0 && searchQuery.trim()}
        <p class="text-sm text-text-muted">No matching fields.</p>
      {/if}

      {#if filtered.tips.length > 0}
        <div class="mt-6 pt-4 border-t border-border">
          <h3 class="text-sm font-semibold text-text-primary mb-2">Race-Day Tips</h3>
          <ul class="list-disc list-inside space-y-1">
            {#each filtered.tips as tip}
              <li class="text-sm text-text-secondary">{@html tip}</li>
            {/each}
          </ul>
        </div>
      {/if}

      {#if section.seeAlso?.length && onNavigate}
        <div class="mt-6 pt-4 border-t border-border">
          <h3 class="text-sm font-semibold text-text-primary mb-2">See Also</h3>
          {#each section.seeAlso as link}
            <button
              onclick={() => {
                if (onNavigate) {
                  onNavigate(link.sectionKey);
                }
              }}
              class="text-sm text-accent hover:underline cursor-pointer bg-transparent border-none mr-4"
              type="button"
            >
              {link.label}
            </button>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</dialog>
