<script lang="ts">
  import { cycleTheme, themeMode } from "../lib/dark-mode";

  export let appName: string = "";
  export let links: Array<{ href: string; label: string; active?: boolean }> =
    [];

  const labels = { light: "Day", dark: "Night", auto: "Auto" } as const;
</script>

<header class="bg-surface-1 border-b border-border">
  <div
    class="mx-auto flex items-center h-12 gap-8 px-6"
    style="max-width: 1100px;"
  >
    <span class="text-sm font-bold tracking-tight text-text-primary"
      >Rusty Timer</span
    >

    {#if links.length > 0}
      <nav class="flex gap-1 h-full">
        {#each links as link}
          <a
            href={link.href}
            class="flex items-center px-3 text-sm font-medium h-full no-underline
              {link.active
              ? 'text-accent border-b-2 border-accent'
              : 'text-text-secondary border-b-2 border-transparent hover:text-text-primary'}"
          >
            {link.label}
          </a>
        {/each}
      </nav>
    {/if}

    <div class="ml-auto flex items-center gap-3">
      <slot name="status" />

      {#if appName}
        <span class="text-xs text-text-muted font-mono">{appName}</span>
      {/if}

      <button
        on:click={cycleTheme}
        class="p-1.5 rounded-md bg-surface-2 border border-border text-text-secondary text-xs cursor-pointer hover:bg-surface-3 flex items-center gap-1.5"
        aria-label="Toggle theme"
      >
        {#if $themeMode === "light"}
          <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
        {:else if $themeMode === "dark"}
          <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>
        {:else}
          <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 3a9 9 0 0 0 0 18z" fill="currentColor"/></svg>
        {/if}
        {labels[$themeMode]}
      </button>
    </div>
  </div>
</header>
