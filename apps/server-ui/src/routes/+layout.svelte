<script lang="ts">
  import "@rusty-timer/shared-ui/styles/tokens.css";
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
