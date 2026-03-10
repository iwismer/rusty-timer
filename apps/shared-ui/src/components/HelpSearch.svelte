<script lang="ts">
  import { searchHelp } from "../lib/help/index";
  import type { HelpContextName } from "../lib/help/help-types";
  import HelpDialog from "./HelpDialog.svelte";

  let {
    context = undefined as HelpContextName | undefined,
  }: {
    context?: HelpContextName;
  } = $props();

  let searchOpen = $state(false);
  let query = $state("");
  let inputEl: HTMLInputElement | undefined = $state();

  let dialogOpen = $state(false);
  let dialogSection = $state("");
  let dialogContext = $state<HelpContextName>("forwarder");
  let dialogField = $state<string | undefined>(undefined);

  let results = $derived(
    query.trim()
      ? searchHelp(query).filter((r) => !context || r.context === context)
      : [],
  );

  function toggleSearch() {
    searchOpen = !searchOpen;
    if (searchOpen) {
      query = "";
      // setTimeout(0) defers focus until after Svelte conditionally renders the input element
      setTimeout(() => inputEl?.focus(), 0);
    }
  }

  function openResult(
    result: (typeof results)[number],
    fieldKey: string,
  ) {
    dialogSection = result.sectionKey;
    dialogContext = result.context;
    dialogField = fieldKey;
    dialogOpen = true;
    searchOpen = false;
    query = "";
  }

  function handleSearchKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      searchOpen = false;
      query = "";
    }
  }

  function handleClickOutside(e: MouseEvent) {
    const target = e.target;
    if (!(target instanceof Element)) {
      searchOpen = false;
      query = "";
      return;
    }
    if (!target.closest("[data-help-search]")) {
      searchOpen = false;
      query = "";
    }
  }

  $effect(() => {
    if (searchOpen) {
      document.addEventListener("click", handleClickOutside);
      return () => document.removeEventListener("click", handleClickOutside);
    }
  });
</script>

<div class="relative" data-help-search>
  <button
    onclick={toggleSearch}
    class="p-1.5 rounded-md bg-surface-2 border border-border text-text-secondary text-xs cursor-pointer hover:bg-surface-3 flex items-center gap-1.5"
    aria-label="Search help"
    type="button"
  >
    <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
    Help
  </button>

  {#if searchOpen}
    <div class="absolute right-0 top-full mt-2 w-96 bg-surface-1 border border-border rounded-lg shadow-lg z-50 overflow-hidden">
      <input
        bind:this={inputEl}
        type="text"
        bind:value={query}
        placeholder="Search help..."
        onkeydown={handleSearchKeydown}
        class="w-full px-4 py-2 text-sm border-b border-border bg-surface-0 text-text-primary placeholder:text-text-muted focus:outline-none"
      />
      <div class="max-h-80 overflow-y-auto">
        {#each results as result}
          <div class="px-4 py-2 border-b border-border last:border-b-0">
            <h4 class="text-xs font-semibold text-text-muted m-0 mb-1">
              {result.section.title}
              <span class="font-normal ml-1">({result.context})</span>
            </h4>
            {#each result.matchedFields as { fieldKey, field }}
              <button
                onclick={() => openResult(result, fieldKey)}
                class="block w-full text-left py-1 hover:bg-surface-2 rounded px-2 cursor-pointer bg-transparent border-none"
                type="button"
              >
                <span class="text-sm text-text-primary">{field.label}</span>
                <span class="text-xs text-text-muted ml-2">{field.summary}</span>
              </button>
            {/each}
          </div>
        {/each}
        {#if query.trim() && results.length === 0}
          <p class="px-4 py-3 text-sm text-text-muted m-0">No results found.</p>
        {/if}
      </div>
    </div>
  {/if}
</div>

<HelpDialog
  open={dialogOpen}
  sectionKey={dialogSection}
  context={dialogContext}
  scrollToField={dialogField}
  onClose={() => { dialogOpen = false; }}
/>
