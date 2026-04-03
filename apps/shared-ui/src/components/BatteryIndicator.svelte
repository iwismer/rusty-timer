<script lang="ts">
  interface Props {
    percent: number | null;
    charging?: boolean;
    available?: boolean;
    configured?: boolean;
    compact?: boolean;
  }

  let { percent, charging = false, available = true, configured = true, compact = false }: Props = $props();

  const colorClass = $derived(
    !configured || percent == null
      ? "text-gray-400"
      : !available
        ? "text-gray-400"
        : percent > 50
          ? "text-green-500"
          : percent > 20
            ? "text-yellow-500"
            : "text-red-500"
  );

  const fillWidth = $derived(
    percent != null ? Math.max(0, Math.min(100, percent)) : 0
  );

  const label = $derived(
    !configured
      ? "—"
      : !available
        ? "UPS unavailable"
        : percent != null
          ? `${percent}%`
          : "—"
  );
</script>

{#if !configured}
  <span class="text-gray-400" title="No UPS configured">—</span>
{:else}
  <span
    class="inline-flex items-center gap-1 {colorClass}"
    title={!available ? "UPS unavailable" : `${percent ?? 0}%${charging ? " (charging)" : ""}`}
  >
    <svg
      class={compact ? "w-4 h-4" : "w-5 h-5"}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
    >
      <rect x="2" y="6" width="18" height="12" rx="2" />
      <path d="M22 10v4" stroke-linecap="round" />
      <rect
        x="4"
        y="8"
        width={14 * fillWidth / 100}
        height="8"
        rx="1"
        fill="currentColor"
        opacity="0.6"
      />
      {#if charging}
        <path d="M12 7l-2 5h4l-2 5" stroke="currentColor" stroke-width="1.5" fill="none" />
      {/if}
    </svg>
    {#if !compact}
      <span class="text-sm">{label}</span>
    {/if}
  </span>
{/if}
