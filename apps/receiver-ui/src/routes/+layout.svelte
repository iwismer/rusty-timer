<script>
  import { onMount } from "svelte";
  import { NavBar } from "@rusty-timer/shared-ui";
  import { initDarkMode } from "@rusty-timer/shared-ui/lib/dark-mode";
  import "@rusty-timer/shared-ui/styles/tokens.css";

  let { children } = $props();

  onMount(() => initDarkMode());
</script>

<svelte:head>
  <title>Receiver · Rusty Timer</title>
</svelte:head>

<div class="flex flex-col min-h-screen min-h-[100dvh]">
  <NavBar links={[{ href: "/", label: "Receiver", active: true }]} />

  <div class="grow">
    {@render children()}
  </div>

  <footer class="border-t border-border py-3 px-8 text-center">
    <p class="text-xs text-text-muted m-0">Rusty Timer &middot; Receiver</p>
  </footer>
</div>
