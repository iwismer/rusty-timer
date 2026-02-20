<script lang="ts">
  let {
    label,
    state = "ok",
  }: {
    label: string;
    state?: "ok" | "warn" | "err";
  } = $props();

  const styles: Record<string, string> = {
    ok: "bg-status-ok-bg text-status-ok border-status-ok-border",
    warn: "bg-status-warn-bg text-status-warn border-status-warn-border",
    err: "bg-status-err-bg text-status-err border-status-err-border",
  };

  const dotStyles: Record<string, string> = {
    ok: "bg-status-ok",
    warn: "bg-status-warn",
    err: "bg-status-err",
  };
</script>

<span
  class="inline-flex items-center gap-1.5 text-xs font-medium px-2 py-0.5 rounded-full border {styles[
    state
  ]}"
>
  <span class="w-1.5 h-1.5 rounded-full {dotStyles[state]}"></span>
  {label}
</span>
