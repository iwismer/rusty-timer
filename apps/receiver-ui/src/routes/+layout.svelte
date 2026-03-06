<script>
  import { onMount } from "svelte";
  import { NavBar } from "@rusty-timer/shared-ui";
  import { initDarkMode } from "@rusty-timer/shared-ui/lib/dark-mode";
  import { page } from "$app/state";
  import "@rusty-timer/shared-ui/styles/tokens.css";

  let { children } = $props();
  let version = $state("");

  onMount(() => {
    initDarkMode();
    fetch("/api/v1/version")
      .then((r) => r.json())
      .then((d) => {
        version = d.version;
      })
      .catch(() => {});
  });
</script>

<svelte:head>
  <title>Receiver · Rusty Timer</title>
</svelte:head>

<div class="flex flex-col min-h-screen min-h-[100dvh]">
  <NavBar
    links={[
      {
        href: "/",
        label: "Receiver",
        active: page.url.pathname === "/",
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

  <footer class="border-t border-border py-3 px-8 text-center">
    <p class="text-xs text-text-muted m-0">
      Rusty Timer &middot; Receiver{version ? ` · v${version}` : ""} &middot; Built
      {__BUILD_DATE__}
    </p>
  </footer>
</div>
