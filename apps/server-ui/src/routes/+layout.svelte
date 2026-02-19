<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { initSSE, destroySSE } from "$lib/sse";

  onMount(() => {
    initSSE();
  });

  onDestroy(() => {
    destroySSE();
  });
</script>

<slot />
