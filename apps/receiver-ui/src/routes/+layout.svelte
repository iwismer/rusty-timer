<script>
  import { onMount, onDestroy } from "svelte";
  import {
    initStore,
    destroyStore,
    store,
    handleDownloadUpdate,
    handleApplyUpdate,
  } from "$lib/store.svelte";
  import { initDarkMode } from "@rusty-timer/shared-ui/lib/dark-mode";
  import { UpdateBanner, AlertBanner } from "@rusty-timer/shared-ui";
  import Toolbar from "$lib/components/Toolbar.svelte";
  import TabBar from "$lib/components/TabBar.svelte";
  import StatusBar from "$lib/components/StatusBar.svelte";
  import HelpModal from "$lib/components/HelpModal.svelte";
  import StreamsTab from "$lib/components/StreamsTab.svelte";
  import ConfigTab from "$lib/components/ConfigTab.svelte";
  import ModeTab from "$lib/components/ModeTab.svelte";
  import LogsTab from "$lib/components/LogsTab.svelte";
  import AdminTab from "$lib/components/AdminTab.svelte";
  import "@rusty-timer/shared-ui/styles/tokens.css";

  let previousHtmlScrollbarGutter = "";
  let previousBodyScrollbarGutter = "";

  onMount(() => {
    previousHtmlScrollbarGutter =
      document.documentElement.style.scrollbarGutter;
    previousBodyScrollbarGutter = document.body.style.scrollbarGutter;
    document.documentElement.style.scrollbarGutter = "auto";
    document.body.style.scrollbarGutter = "auto";
    initDarkMode();
    initStore();
  });

  onDestroy(() => {
    document.documentElement.style.scrollbarGutter =
      previousHtmlScrollbarGutter;
    document.body.style.scrollbarGutter = previousBodyScrollbarGutter;
    destroyStore();
  });
</script>

<svelte:head>
  <title>Receiver &middot; Rusty Timer</title>
</svelte:head>

<div class="flex flex-col h-screen">
  <Toolbar />
  <TabBar />

  {#if store.updateVersion && store.updateStatus}
    <div class="px-3 py-1.5 shrink-0">
      <UpdateBanner
        version={store.updateVersion}
        status={store.updateStatus}
        busy={store.updateBusy}
        onDownload={handleDownloadUpdate}
        onApply={handleApplyUpdate}
      />
    </div>
  {/if}

  {#if store.error}
    <div class="px-3 py-1.5 shrink-0">
      <AlertBanner variant="err" message={store.error} />
    </div>
  {/if}

  <div class="flex-1 overflow-y-auto">
    {#if store.activeTab === "streams"}
      <StreamsTab />
    {:else if store.activeTab === "config"}
      <ConfigTab />
    {:else if store.activeTab === "mode"}
      <ModeTab />
    {:else if store.activeTab === "logs"}
      <LogsTab />
    {:else if store.activeTab === "admin"}
      <AdminTab />
    {/if}
  </div>

  <StatusBar />
  <HelpModal />
</div>
