<script lang="ts">
  import type { Snippet } from "svelte";

  let { children }: { children?: Snippet } = $props();
</script>

<div class="overflow-x-auto rounded-md border border-border">
  <table class="w-full text-sm border-collapse">
    {#if children}
      {@render children()}
    {/if}
  </table>
</div>
