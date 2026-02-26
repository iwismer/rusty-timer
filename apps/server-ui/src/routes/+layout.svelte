<script lang="ts">
  import type { Snippet } from "svelte";
  import "@rusty-timer/shared-ui/styles/tokens.css";
  import { onMount, onDestroy } from "svelte";
  import { initSSE, destroySSE } from "$lib/sse";
  import { initDarkMode } from "@rusty-timer/shared-ui/lib/dark-mode";
  import { NavBar } from "@rusty-timer/shared-ui";
  import { getRaces, getForwarderRaces } from "$lib/api";
  import { setRaces, forwarderRacesStore } from "$lib/stores";
  import { page } from "$app/state";

  let { children }: { children: Snippet } = $props();

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

<div class="flex flex-col min-h-screen min-h-[100dvh]">
  <NavBar
    links={[
      {
        href: "/",
        label: "Streams",
        active:
          page.url.pathname === "/" ||
          page.url.pathname.startsWith("/streams") ||
          page.url.pathname.startsWith("/forwarders"),
      },
      {
        href: "/races",
        label: "Races",
        active: page.url.pathname.startsWith("/races"),
      },
      {
        href: "/announcer-config",
        label: "Announcer",
        active: page.url.pathname.startsWith("/announcer-config"),
      },
      {
        href: "/logs",
        label: "Logs",
        active: page.url.pathname.startsWith("/logs"),
      },
      {
        href: "/admin",
        label: "Admin",
        active: page.url.pathname.startsWith("/admin"),
      },
    ]}
  />

  <div class="grow">
    {@render children()}
  </div>

  <footer class="border-t border-border py-3 px-6 text-center">
    <p class="text-xs text-text-muted m-0">Rusty Timer &middot; Server</p>
  </footer>
</div>
