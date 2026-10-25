<script lang="ts">
  import { ForwarderWifiManager } from "@rusty-timer/shared-ui";
  import { wifiApi } from "$lib/api";
</script>

<main class="max-w-[900px] mx-auto px-6 py-6">
  <h1 class="text-xl font-bold text-text-primary mb-6">Wi-Fi Networks</h1>
  <ForwarderWifiManager {wifiApi} />
</main>
