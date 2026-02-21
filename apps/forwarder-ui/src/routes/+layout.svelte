<script>
  import { onMount } from "svelte";
  import { page } from "$app/stores";
  import { NavBar } from "@rusty-timer/shared-ui";
  import { initDarkMode } from "@rusty-timer/shared-ui/lib/dark-mode";
  import "@rusty-timer/shared-ui/styles/tokens.css";

  let { children } = $props();

  onMount(() => initDarkMode());

  let currentPath = $derived($page.url.pathname);
</script>

<svelte:head>
  <title>Forwarder · Rusty Timer</title>
</svelte:head>

<div class="flex flex-col min-h-screen min-h-[100dvh]">
  <NavBar
    links={[
      { href: "/", label: "Status", active: currentPath === "/" },
      { href: "/config", label: "Config", active: currentPath === "/config" },
    ]}
  />

  <div class="grow">
    {@render children()}
  </div>

  <footer class="border-t border-border py-3 px-6 text-center">
    <p class="text-xs text-text-muted m-0">Rusty Timer &middot; Forwarder</p>
  </footer>
</div>
