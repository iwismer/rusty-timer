<script lang="ts">
  import { onMount } from "svelte";
  import { page } from "$app/stores";
  import { NavBar } from "@rusty-timer/shared-ui";
  import { initDarkMode } from "@rusty-timer/shared-ui/lib/dark-mode";
  import "@rusty-timer/shared-ui/styles/tokens.css";

  let { children } = $props();

  onMount(() => {
    initDarkMode();
  });

  let currentPath = $derived($page.url.pathname);
  // The announcer page is a standalone public display: no app nav/footer chrome.
  let isStandalone = $derived(currentPath === "/announcer");
</script>

<svelte:head>
  <title>Server · Rusty Timer</title>
</svelte:head>

{#if isStandalone}
  {@render children()}
{:else}
  <div class="flex flex-col min-h-screen min-h-[100dvh]">
    <NavBar
      appName="Server"
      links={[
        { href: "/", label: "Status", active: currentPath === "/" },
        {
          href: "/sbc-setup",
          label: "SBC Setup",
          active: currentPath === "/sbc-setup",
        },
        { href: "/admin", label: "Admin", active: currentPath === "/admin" },
      ]}
    />

    <main class="grow">
      {@render children()}
    </main>

    <footer class="border-t border-border py-3 px-6 text-center">
      <p class="text-xs text-text-muted m-0">
        Rusty Timer &middot; Server &middot; Built {__BUILD_DATE__}
      </p>
    </footer>
  </div>
{/if}
