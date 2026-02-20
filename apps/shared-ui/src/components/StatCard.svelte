<script lang="ts">
  let {
    label,
    value,
    subtitle = undefined,
    muted = false,
  }: {
    label: string;
    value: string | number;
    subtitle?: string;
    muted?: boolean;
  } = $props();
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
