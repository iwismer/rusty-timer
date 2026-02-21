<script lang="ts">
  import type { ConfigApi } from "../lib/config-types";
  import ForwarderConfig from "./ForwarderConfig.svelte";

  let {
    configApi,
    pageTitle = "Config",
    displayName = undefined,
    isOnline = undefined,
    backHref = undefined,
    backLabel = "Back",
  }: {
    configApi: ConfigApi;
    pageTitle?: string;
    displayName?: string;
    isOnline?: boolean;
    backHref?: string;
    backLabel?: string;
  } = $props();
</script>

<main class="max-w-[900px] mx-auto px-6 py-6">
  {#if backHref}
    <div class="mb-4">
      <a href={backHref} class="text-xs text-accent no-underline hover:underline">
        &larr; {backLabel}
      </a>
    </div>
  {/if}

  {#if displayName === undefined}
    <h1 class="text-xl font-bold text-text-primary mb-6">{pageTitle}</h1>
  {/if}

  <ForwarderConfig {configApi} {displayName} {isOnline} />
</main>
