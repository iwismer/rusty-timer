<script lang="ts">
  import type { Snippet } from "svelte";
  import { onMount, onDestroy } from "svelte";
  import { page } from "$app/state";
  import { initStore, destroyStore, store } from "$lib/store.svelte";
  import { initDarkMode } from "@rusty-timer/shared-ui/lib/dark-mode";
  import { AlertBanner } from "@rusty-timer/shared-ui";
  import TabBar from "$lib/components/TabBar.svelte";
  import StatusBar from "$lib/components/StatusBar.svelte";
  import HelpModal from "$lib/components/HelpModal.svelte";
  import UpdateModal from "$lib/components/UpdateModal.svelte";
  import StreamsTab from "$lib/components/StreamsTab.svelte";
  import ForwardersTab from "$lib/components/ForwardersTab.svelte";
  import AnnouncerTab from "$lib/components/AnnouncerTab.svelte";
  import RacesTab from "$lib/components/RacesTab.svelte";
  import ConfigTab from "$lib/components/ConfigTab.svelte";
  import ModeTab from "$lib/components/ModeTab.svelte";
  import LogsTab from "$lib/components/LogsTab.svelte";
  import AdminTab from "$lib/components/AdminTab.svelte";
  import "@rusty-timer/shared-ui/styles/tokens.css";

  let { children }: { children?: Snippet } = $props();
  let previousHtmlScrollbarGutter = "";
  let previousBodyScrollbarGutter = "";
  let hasNestedRoute = $derived(page.url.pathname !== "/");

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
  <TabBar />

  {#if store.error}
    <div class="px-3 py-1.5 shrink-0">
      <AlertBanner variant="err" message={store.error} />
    </div>
  {/if}

  <div class="flex-1 overflow-y-auto">
    {#if hasNestedRoute && children}
      {@render children()}
    {:else if store.activeTab === "streams"}
      <StreamsTab />
    {:else if store.activeTab === "forwarders"}
      <ForwardersTab />
    {:else if store.activeTab === "announcer"}
      <AnnouncerTab />
    {:else if store.activeTab === "races"}
      <RacesTab />
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
  <UpdateModal />
  <HelpModal />
</div>
