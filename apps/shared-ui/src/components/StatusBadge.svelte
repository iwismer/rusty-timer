<script lang="ts">
  export let label: string;
  export let state: "ok" | "warn" | "err" = "ok";
</script>

<span class="badge {state}">{label}</span>

<style>
  .badge {
    padding: 0.2rem 0.5rem;
    border-radius: 4px;
    font-size: 0.85em;
    display: inline-block;
  }
  .ok {
    background: #d4edda;
    color: #155724;
  }
  .warn {
    background: #fff3cd;
    color: #856404;
  }
  .err {
    background: #f8d7da;
    color: #721c24;
  }
</style>
