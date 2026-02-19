<script lang="ts">
  import "@rusty-timer/shared-ui/styles/tokens.css";
  import { onMount, onDestroy } from "svelte";
  import { initSSE, destroySSE } from "$lib/sse";
  import { initDarkMode } from "@rusty-timer/shared-ui/lib/dark-mode";
  import { NavBar } from "@rusty-timer/shared-ui";

  onMount(() => {
    initSSE();
    initDarkMode();
  });

  onDestroy(() => {
    destroySSE();
  });
</script>

<NavBar links={[{ href: "/", label: "Streams", active: true }]} />

<slot />

<footer class="border-t border-border py-3 px-6 text-center">
  <p class="text-xs text-text-muted m-0">Rusty Timer &middot; Server</p>
</footer>
