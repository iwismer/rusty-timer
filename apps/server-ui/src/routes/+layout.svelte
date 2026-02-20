<script lang="ts">
  import type { Snippet } from "svelte";
  import "@rusty-timer/shared-ui/styles/tokens.css";
  import { onMount, onDestroy } from "svelte";
  import { initSSE, destroySSE } from "$lib/sse";
  import { initDarkMode } from "@rusty-timer/shared-ui/lib/dark-mode";
  import { NavBar } from "@rusty-timer/shared-ui";

  let { children }: { children: Snippet } = $props();

  onMount(() => {
    initSSE();
    initDarkMode();
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
    { href: "/", label: "Streams" },
    { href: "/races", label: "Races" },
  ]}
/>

{@render children()}

<footer class="border-t border-border py-3 px-6 text-center">
  <p class="text-xs text-text-muted m-0">Rusty Timer &middot; Server</p>
</footer>
