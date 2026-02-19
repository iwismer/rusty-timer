<script>
  import "@rusty-timer/shared-ui/styles/tokens.css";
</script>

<nav>
  <a href="/">Status</a>
  <a href="/config">Config</a>
</nav>

<slot />

<style>
  nav {
    display: flex;
    gap: 1rem;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid #ddd;
    max-width: 800px;
    margin: 0 auto;
  }
</style>
