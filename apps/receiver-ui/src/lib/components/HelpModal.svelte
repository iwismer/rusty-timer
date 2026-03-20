<script lang="ts">
  import {
    store,
    setShowHelpModal,
    setHelpScrollTarget,
  } from "$lib/store.svelte";
  import { RECEIVER_HELP } from "@rusty-timer/shared-ui/lib/help/receiver-help";
  import { RECEIVER_ADMIN_HELP } from "@rusty-timer/shared-ui/lib/help/receiver-admin-help";
  import type {
    SectionHelp,
    FieldHelp,
  } from "@rusty-timer/shared-ui/lib/help/help-types";
  import { tick } from "svelte";

  let searchQuery = $state("");

  type HelpSection = { key: string; section: SectionHelp; context: string };

  function allSections(): HelpSection[] {
    const sections: HelpSection[] = [];
    for (const [key, section] of Object.entries(RECEIVER_HELP)) {
      sections.push({
        key,
        section: section as SectionHelp,
        context: "receiver",
      });
    }
    for (const [key, section] of Object.entries(RECEIVER_ADMIN_HELP)) {
      sections.push({
        key,
        section: section as SectionHelp,
        context: "receiver-admin",
      });
    }
    return sections;
  }

  function filteredSections(): HelpSection[] {
    const q = searchQuery.trim().toLowerCase();
    if (!q) return allSections();
    return allSections().filter(({ section }) => {
      if (section.title.toLowerCase().includes(q)) return true;
      if (section.overview.toLowerCase().includes(q)) return true;
      for (const field of Object.values(section.fields)) {
        const f = field as FieldHelp;
        if (f.label.toLowerCase().includes(q)) return true;
        if (f.summary.toLowerCase().includes(q)) return true;
      }
      return false;
    });
  }

  function close() {
    setShowHelpModal(false);
    setHelpScrollTarget(null);
    searchQuery = "";
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") close();
  }

  let dialogRef: HTMLDivElement | undefined = $state(undefined);
  let cachedSections = $derived(filteredSections());

  $effect(() => {
    if (store.showHelpModal) {
      void tick().then(() => {
        // Move focus into the modal on open.
        dialogRef?.focus();
        if (store.helpScrollTarget) {
          const el = document.getElementById(`help-${store.helpScrollTarget}`);
          el?.scrollIntoView({ behavior: "smooth", block: "start" });
        }
      });
    }
  });
</script>

{#if store.showHelpModal}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
    bind:this={dialogRef}
    onclick={close}
    onkeydown={handleKeydown}
    role="dialog"
    aria-modal="true"
    tabindex="-1"
  >
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="bg-surface-0 rounded-lg shadow-xl w-full max-w-[600px] max-h-[80vh] flex flex-col mx-4"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
    >
      <div
        class="flex items-center justify-between px-4 py-3 border-b border-border shrink-0"
      >
        <h2 class="text-sm font-semibold text-text-primary m-0">Help</h2>
        <button
          onclick={close}
          class="text-text-muted hover:text-text-primary text-lg leading-none cursor-pointer bg-transparent border-none"
          aria-label="Close"
        >
          &times;
        </button>
      </div>

      <div class="px-4 py-2 border-b border-border shrink-0">
        <input
          type="text"
          placeholder="Search help..."
          bind:value={searchQuery}
          class="w-full px-3 py-1.5 text-sm rounded-md bg-surface-1 border border-border text-text-primary focus:outline-none focus:ring-1 focus:ring-accent"
        />
      </div>

      <div class="overflow-y-auto px-4 py-3 flex-1">
        {#each cachedSections as { key, section, context } (context + "/" + key)}
          <div id="help-{key}" class="mb-6">
            <h3 class="text-sm font-semibold text-text-primary mb-1">
              {section.title}
            </h3>
            <p class="text-xs text-text-muted mb-3">{section.overview}</p>

            {#each Object.entries(section.fields) as [fieldKey, field] (fieldKey)}
              {@const f = field as FieldHelp}
              <div
                id="help-{fieldKey}"
                class="mb-3 pl-3 border-l-2 border-border"
              >
                <h4 class="text-xs font-semibold text-text-primary mb-0.5">
                  {f.label}
                </h4>
                <p class="text-xs text-text-muted m-0 mb-1">{f.summary}</p>
                <div class="text-xs text-text-secondary">
                  {@html f.detailHtml}
                </div>
                {#if f.default}
                  <p class="text-xs text-text-muted m-0 mt-1">
                    <span class="font-medium">Default:</span>
                    {f.default}
                  </p>
                {/if}
                {#if f.recommended}
                  <p class="text-xs text-text-muted m-0 mt-0.5">
                    <span class="font-medium">Recommended:</span>
                    {f.recommended}
                  </p>
                {/if}
              </div>
            {/each}

            {#if section.tips && section.tips.length > 0}
              <div class="mt-2">
                <h4 class="text-xs font-semibold text-text-muted mb-1">Tips</h4>
                <ul class="list-disc pl-4 text-xs text-text-muted space-y-0.5">
                  {#each section.tips as tip}
                    <li>{@html tip}</li>
                  {/each}
                </ul>
              </div>
            {/if}
          </div>
        {/each}

        {#if cachedSections.length === 0}
          <p class="text-sm text-text-muted text-center py-8">
            No results for "{searchQuery}"
          </p>
        {/if}
      </div>
    </div>
  </div>
{/if}
