<script lang="ts">
  import { toggleDarkMode } from "../lib/dark-mode";

  export let appName: string = "";
  export let links: Array<{ href: string; label: string; active?: boolean }> =
    [];
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
        on:click={toggleDarkMode}
        class="p-1.5 rounded-md bg-surface-2 border border-border text-text-secondary text-xs cursor-pointer hover:bg-surface-3"
        aria-label="Toggle dark mode"
      >
        Theme
      </button>
    </div>
  </div>
</header>
