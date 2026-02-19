<script lang="ts">
  export let label: string;
  export let value: string | number;
  export let subtitle: string | undefined = undefined;
  export let muted: boolean = false;
</script>

<div>
  <p class="text-xs text-text-muted">{label}</p>
  <p
    class="text-lg font-bold font-mono {muted
      ? 'text-text-muted'
      : 'text-text-primary'}"
  >
    {value}
  </p>
  {#if subtitle}
    <p class="text-xs text-text-secondary mt-0.5">{subtitle}</p>
  {/if}
</div>
