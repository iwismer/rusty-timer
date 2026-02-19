<script lang="ts">
  export let version: string;
  export let busy: boolean = false;
  export let onApply: () => void;
</script>

<section class="update-banner" data-testid="update-banner">
  <p>Update v{version} available</p>
  <button
    data-testid="apply-update-btn"
    on:click={onApply}
    disabled={busy}
  >
    {busy ? "Applying..." : "Update Now"}
  </button>
</section>

<style>
  .update-banner {
    background: #d4edda;
    border: 1px solid #c3e6cb;
    border-radius: 4px;
    padding: 1rem;
    margin-bottom: 1rem;
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .update-banner p {
    margin: 0;
    font-weight: 600;
    color: #155724;
  }
  button {
    padding: 0.5rem 1rem;
    cursor: pointer;
    border: 1px solid #155724;
    border-radius: 4px;
    background: #fff;
  }
  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
