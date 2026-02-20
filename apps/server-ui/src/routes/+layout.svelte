<script lang="ts">
  import type { Snippet } from "svelte";
  import { page } from "$app/stores";
  import "@rusty-timer/shared-ui/styles/tokens.css";
  import { onMount, onDestroy } from "svelte";
  import { initSSE, destroySSE } from "$lib/sse";
  import { initDarkMode } from "@rusty-timer/shared-ui/lib/dark-mode";
  import { NavBar } from "@rusty-timer/shared-ui";
  import { getRaces, getForwarderRaces } from "$lib/api";
  import { setRaces, forwarderRacesStore } from "$lib/stores";

  let { children }: { children: Snippet } = $props();
  let currentPath = $derived($page.url.pathname);

  onMount(() => {
    initSSE();
    initDarkMode();

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
  });

  onDestroy(() => {
    destroySSE();
  });
</script>

<svelte:head>
  <title>Server · Rusty Timer</title>
</svelte:head>

<NavBar
  links={[
    { href: "/", label: "Streams", active: currentPath === "/" },
    {
      href: "/races",
      label: "Races",
      active: currentPath.startsWith("/races"),
    },
  ]}
/>

{@render children()}

<footer class="border-t border-border py-3 px-6 text-center">
  <p class="text-xs text-text-muted m-0">Rusty Timer &middot; Server</p>
</footer>
