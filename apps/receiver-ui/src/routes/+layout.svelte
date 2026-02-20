<script>
  import { onMount } from "svelte";
  import { NavBar } from "@rusty-timer/shared-ui";
  import { initDarkMode } from "@rusty-timer/shared-ui/lib/dark-mode";
  import "@rusty-timer/shared-ui/styles/tokens.css";

  onMount(() => initDarkMode());
</script>

<svelte:head>
  <title>Receiver · Rusty Timer</title>
</svelte:head>

<NavBar links={[{ href: "/", label: "Receiver", active: true }]} />

<slot />

<footer class="border-t border-border py-3 px-8 text-center">
  <p class="text-xs text-text-muted m-0">Rusty Timer &middot; Receiver</p>
</footer>
