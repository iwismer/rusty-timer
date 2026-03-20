<script lang="ts">
  import { store, setActiveTab } from "$lib/store.svelte";
  import type { TabId } from "$lib/store.svelte";

  const tabs: { id: TabId; label: string }[] = [
    { id: "streams", label: "Streams" },
    { id: "config", label: "Config" },
    { id: "mode", label: "Mode" },
    { id: "logs", label: "Logs" },
    { id: "admin", label: "Admin" },
  ];
</script>

<div
  class="flex items-end gap-1 px-2 bg-surface-0 border-b border-border shrink-0"
>
  {#each tabs as tab (tab.id)}
    <button
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
