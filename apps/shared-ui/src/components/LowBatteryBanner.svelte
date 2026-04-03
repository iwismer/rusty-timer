<script lang="ts">
  interface LowBatteryForwarder {
    name: string;
    percent: number;
  }

  interface Props {
    forwarders: LowBatteryForwarder[];
    onDismiss?: () => void;
  }

  let { forwarders, onDismiss }: Props = $props();
</script>

{#if forwarders.length > 0}
  <div class="bg-red-600 text-white px-4 py-2 flex items-center justify-between text-sm rounded">
    <span>
      Low battery:
      {#each forwarders as fwd, i}
        {fwd.name} at {fwd.percent}%{i < forwarders.length - 1 ? ", " : ""}
      {/each}
    </span>
    {#if onDismiss}
      <button
        class="ml-4 text-white/80 hover:text-white"
        onclick={onDismiss}
        aria-label="Dismiss"
      >
        ✕
      </button>
    {/if}
  </div>
{/if}
