<script lang="ts">
  import { store, setActiveTab, openRaceDayWizard } from "$lib/store.svelte";
  import type { TabId } from "$lib/store.svelte";

  const tabs: { id: TabId; label: string }[] = [
    { id: "streams", label: "Streams" },
    { id: "connections", label: "Forwarders" },
    { id: "config", label: "Config" },
    { id: "announcer", label: "Announcer" },
    { id: "logs", label: "Logs" },
    { id: "admin", label: "Admin" },
  ];
</script>

<div
  class="flex items-center justify-between px-2 bg-surface-0 border-b border-border shrink-0"
>
  <div class="flex items-end gap-1" role="tablist">
    {#each tabs as tab (tab.id)}
      <button
        role="tab"
        aria-selected={store.activeTab === tab.id}
        class="px-3 py-1.5 text-sm border-b-2 transition-colors cursor-pointer bg-transparent
          {store.activeTab === tab.id
          ? 'border-accent text-text-primary font-semibold'
          : 'border-transparent text-text-muted hover:text-text-primary'}"
        onclick={() => setActiveTab(tab.id)}
      >
        {tab.label}
      </button>
    {/each}
  </div>

  <div class="flex items-center pr-2 py-1">
    <button
      type="button"
      data-testid="race-day-setup-btn"
      class="flex items-center gap-1.5 px-2.5 py-1 text-xs font-semibold rounded-md bg-accent hover:bg-accent-hover text-white cursor-pointer shadow-sm transition-colors"
      onclick={() => openRaceDayWizard()}
    >
      <svg
        class="w-3.5 h-3.5"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <path d="M4 15s1-1 4-1 5 2 8 2 4-1 4-1V3s-1 1-4 1-5-2-8-2-4 1-4 1z" />
        <line x1="4" y1="22" x2="4" y2="15" />
      </svg>
      <span>Race Day Setup</span>
    </button>
  </div>
</div>
