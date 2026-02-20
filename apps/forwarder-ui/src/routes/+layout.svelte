<script>
  import { onMount } from "svelte";
  import { page } from "$app/stores";
  import { NavBar } from "@rusty-timer/shared-ui";
  import { initDarkMode } from "@rusty-timer/shared-ui/lib/dark-mode";
  import "@rusty-timer/shared-ui/styles/tokens.css";

  onMount(() => initDarkMode());

  $: currentPath = $page.url.pathname;
</script>

<svelte:head>
  <title>Forwarder · Rusty Timer</title>
</svelte:head>

<NavBar
  links={[
    { href: "/", label: "Status", active: currentPath === "/" },
    { href: "/config", label: "Config", active: currentPath === "/config" },
  ]}
/>

<slot />

<footer class="border-t border-border py-3 px-6 text-center mt-8">
  <p class="text-xs text-text-muted m-0">Rusty Timer &middot; Forwarder</p>
</footer>
