<script lang="ts">
  import type { Snippet } from "svelte";
  import "@rusty-timer/shared-ui/styles/tokens.css";
  import { onMount } from "svelte";
  import { afterNavigate } from "$app/navigation";
  import { initSSE, destroySSE } from "$lib/sse";
  import { initDarkMode } from "@rusty-timer/shared-ui/lib/dark-mode";
  import { NavBar } from "@rusty-timer/shared-ui";
  import { getRaces, getForwarderRaces } from "$lib/api";
  import { shouldBootstrapDashboard } from "$lib/layout-bootstrap";
  import { getLayoutNavLinks } from "$lib/layout-nav";
  import { setRaces, forwarderRacesStore } from "$lib/stores";
  import { page } from "$app/state";

  let { children }: { children: Snippet } = $props();
  let navLinks = $derived(getLayoutNavLinks(page.url.pathname));

  function loadDashboardReferenceData() {
    // Load races and forwarder-race assignments in parallel
    Promise.all([getRaces(), getForwarderRaces()])
      .then(([racesResp, frResp]) => {
        setRaces(racesResp.races);
        const map: Record<string, string | null> = {};
        for (const a of frResp.assignments) {
          map[a.forwarder_id] = a.race_id;
        }
        forwarderRacesStore.set(map);
      })
      .catch(() => {
        // Silent — SSE will keep things in sync
      });
  }

  onMount(() => {
    initDarkMode();
    let isBootstrapped = false;

    function syncDashboardBootstrap(pathname: string): void {
      const shouldBootstrap = shouldBootstrapDashboard(pathname);
      if (shouldBootstrap && !isBootstrapped) {
        initSSE();
        loadDashboardReferenceData();
        isBootstrapped = true;
      } else if (!shouldBootstrap && isBootstrapped) {
        destroySSE();
        isBootstrapped = false;
      }
    }

    syncDashboardBootstrap(page.url.pathname);
    afterNavigate(() => {
      syncDashboardBootstrap(page.url.pathname);
    });

    return () => {
      if (isBootstrapped) {
        destroySSE();
      }
    };
  });
</script>

<svelte:head>
  <title>Server · Rusty Timer</title>
</svelte:head>

<div class="flex flex-col min-h-screen min-h-[100dvh]">
  <NavBar links={navLinks} />

  <div class="grow">
    {@render children()}
  </div>

  <footer class="border-t border-border py-3 px-6 text-center">
    <p class="text-xs text-text-muted m-0">Rusty Timer &middot; Server</p>
  </footer>
</div>
