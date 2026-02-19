<script lang="ts">
  import { page } from "$app/stores";
  import { onMount } from "svelte";
  import * as api from "$lib/api";
  import { streamsStore } from "$lib/stores";

  const forwarderId = $page.params.forwarderId;
  let config: Record<string, unknown> | null = null;
  let restartNeeded = false;
  let loading = true;
  let loadError: string | null = null;
  let sectionMessages: Record<string, { ok: boolean; text: string }> = {};
  let savingSection: Record<string, boolean> = {};

  // Derive online status from streams store
  $: isOnline = $streamsStore.some(
    (s) => s.forwarder_id === forwarderId && s.online,
  );
  $: displayName =
    $streamsStore.find((s) => s.forwarder_id === forwarderId)
      ?.forwarder_display_name ?? forwarderId;

  // Form field values — extracted from config
  let generalDisplayName = "";

  let serverBaseUrl = "";
  let serverForwardersWsPath = "";

  let authTokenFile = "";

  let journalSqlitePath = "";
  let journalPruneWatermarkPct = "";

  let uplinkBatchMode = "";
  let uplinkBatchFlushMs = "";
  let uplinkBatchMaxEvents = "";

  let statusHttpBind = "";

  interface ReaderEntry {
    target: string;
    enabled: boolean;
    local_fallback_port: string;
  }
  let readers: ReaderEntry[] = [];

  onMount(async () => {
    await loadConfig();
  });

  async function loadConfig() {
    loading = true;
    loadError = null;
    try {
      const resp = await api.getForwarderConfig(forwarderId);
      config = resp.config;
      restartNeeded = resp.restart_needed;
      populateFields(resp.config);
    } catch (e) {
      loadError = String(e);
    } finally {
      loading = false;
    }
  }

  function populateFields(cfg: Record<string, unknown>) {
    generalDisplayName = (cfg.display_name as string) ?? "";

    const server = (cfg.server as Record<string, unknown>) ?? {};
    serverBaseUrl = (server.base_url as string) ?? "";
    serverForwardersWsPath = (server.forwarders_ws_path as string) ?? "";

    const auth = (cfg.auth as Record<string, unknown>) ?? {};
    authTokenFile = (auth.token_file as string) ?? "";

    const journal = (cfg.journal as Record<string, unknown>) ?? {};
    journalSqlitePath = (journal.sqlite_path as string) ?? "";
    journalPruneWatermarkPct =
      journal.prune_watermark_pct != null
        ? String(journal.prune_watermark_pct)
        : "";

    const uplink = (cfg.uplink as Record<string, unknown>) ?? {};
    uplinkBatchMode = (uplink.batch_mode as string) ?? "";
    uplinkBatchFlushMs =
      uplink.batch_flush_ms != null ? String(uplink.batch_flush_ms) : "";
    uplinkBatchMaxEvents =
      uplink.batch_max_events != null ? String(uplink.batch_max_events) : "";

    const statusHttp = (cfg.status_http as Record<string, unknown>) ?? {};
    statusHttpBind = (statusHttp.bind as string) ?? "";

    const rawReaders = (cfg.readers as Record<string, unknown>[]) ?? [];
    readers = rawReaders.map((r) => ({
      target: (r.target as string) ?? "",
      enabled: (r.enabled as boolean) ?? true,
      local_fallback_port:
        r.local_fallback_port != null ? String(r.local_fallback_port) : "",
    }));
  }

  async function saveSection(
    section: string,
    payload: Record<string, unknown>,
  ) {
    sectionMessages[section] = { ok: false, text: "Saving..." };
    savingSection[section] = true;
    try {
      const result = await api.setForwarderConfig(
        forwarderId,
        section,
        payload,
      );
      if (result.ok) {
        sectionMessages[section] = {
          ok: true,
          text: "Saved. Restart to apply.",
        };
        restartNeeded = result.restart_needed;
      } else {
        sectionMessages[section] = {
          ok: false,
          text: result.error ?? "Unknown error",
        };
      }
    } catch (e) {
      sectionMessages[section] = { ok: false, text: String(e) };
    } finally {
      savingSection[section] = false;
    }
  }

  function saveGeneral() {
    saveSection("general", {
      display_name: generalDisplayName || null,
    });
  }

  function saveServer() {
    saveSection("server", {
      base_url: serverBaseUrl,
      forwarders_ws_path: serverForwardersWsPath || null,
    });
  }

  function saveAuth() {
    saveSection("auth", {
      token_file: authTokenFile,
    });
  }

  function saveJournal() {
    saveSection("journal", {
      sqlite_path: journalSqlitePath || null,
      prune_watermark_pct: journalPruneWatermarkPct
        ? Number(journalPruneWatermarkPct)
        : null,
    });
  }

  function saveUplink() {
    saveSection("uplink", {
      batch_mode: uplinkBatchMode || null,
      batch_flush_ms: uplinkBatchFlushMs ? Number(uplinkBatchFlushMs) : null,
      batch_max_events: uplinkBatchMaxEvents
        ? Number(uplinkBatchMaxEvents)
        : null,
    });
  }

  function saveStatusHttp() {
    saveSection("status_http", {
      bind: statusHttpBind || null,
    });
  }

  function saveReaders() {
    saveSection("readers", {
      readers: readers.map((r) => ({
        target: r.target || null,
        enabled: r.enabled,
        local_fallback_port: r.local_fallback_port
          ? Number(r.local_fallback_port)
          : null,
      })),
    });
  }

  function addReader() {
    readers = [
      ...readers,
      { target: "", enabled: true, local_fallback_port: "" },
    ];
  }

  function removeReader(index: number) {
    readers = readers.filter((_, i) => i !== index);
  }
</script>

<main>
  <a href="/">&#8592; Back to streams</a>

  <h1>
    Configure: {displayName}
    {#if isOnline}
      <span class="badge online">online</span>
    {:else}
      <span class="badge offline">offline</span>
    {/if}
  </h1>

  {#if restartNeeded}
    <div class="restart-banner">
      Restart needed — some changes require a forwarder restart to take effect.
    </div>
  {/if}

  {#if loading}
    <p>Loading configuration...</p>
  {:else if loadError}
    <p class="error">{loadError}</p>
    <button on:click={loadConfig}>Retry</button>
  {:else}
    <!-- General -->
    <section class="config-card">
      <h2>General</h2>
      <label>
        Display Name
        <input type="text" bind:value={generalDisplayName} />
      </label>
      <button on:click={saveGeneral} disabled={savingSection["general"]}>
        {savingSection["general"] ? "Saving..." : "Save General"}
      </button>
      {#if sectionMessages["general"]}
        <p
          class:success={sectionMessages["general"].ok}
          class:error={!sectionMessages["general"].ok}
        >
          {sectionMessages["general"].text}
        </p>
      {/if}
    </section>

    <!-- Server -->
    <section class="config-card">
      <h2>Server</h2>
      <label>
        Base URL *
        <input type="text" bind:value={serverBaseUrl} required />
      </label>
      <label>
        Forwarders WS Path
        <input type="text" bind:value={serverForwardersWsPath} />
      </label>
      <button on:click={saveServer} disabled={savingSection["server"]}>
        {savingSection["server"] ? "Saving..." : "Save Server"}
      </button>
      {#if sectionMessages["server"]}
        <p
          class:success={sectionMessages["server"].ok}
          class:error={!sectionMessages["server"].ok}
        >
          {sectionMessages["server"].text}
        </p>
      {/if}
    </section>

    <!-- Auth -->
    <section class="config-card">
      <h2>Auth</h2>
      <label>
        Token File Path *
        <input type="text" bind:value={authTokenFile} required />
      </label>
      <button on:click={saveAuth} disabled={savingSection["auth"]}>
        {savingSection["auth"] ? "Saving..." : "Save Auth"}
      </button>
      {#if sectionMessages["auth"]}
        <p
          class:success={sectionMessages["auth"].ok}
          class:error={!sectionMessages["auth"].ok}
        >
          {sectionMessages["auth"].text}
        </p>
      {/if}
    </section>

    <!-- Journal -->
    <section class="config-card">
      <h2>Journal</h2>
      <label>
        SQLite Path
        <input type="text" bind:value={journalSqlitePath} />
      </label>
      <label>
        Prune Watermark %
        <input
          type="number"
          bind:value={journalPruneWatermarkPct}
          min="0"
          max="100"
        />
      </label>
      <button on:click={saveJournal} disabled={savingSection["journal"]}>
        {savingSection["journal"] ? "Saving..." : "Save Journal"}
      </button>
      {#if sectionMessages["journal"]}
        <p
          class:success={sectionMessages["journal"].ok}
          class:error={!sectionMessages["journal"].ok}
        >
          {sectionMessages["journal"].text}
        </p>
      {/if}
    </section>

    <!-- Uplink -->
    <section class="config-card">
      <h2>Uplink</h2>
      <label>
        Batch Mode
        <input type="text" bind:value={uplinkBatchMode} />
      </label>
      <label>
        Batch Flush (ms)
        <input type="number" bind:value={uplinkBatchFlushMs} min="0" />
      </label>
      <label>
        Batch Max Events
        <input type="number" bind:value={uplinkBatchMaxEvents} min="0" />
      </label>
      <button on:click={saveUplink} disabled={savingSection["uplink"]}>
        {savingSection["uplink"] ? "Saving..." : "Save Uplink"}
      </button>
      {#if sectionMessages["uplink"]}
        <p
          class:success={sectionMessages["uplink"].ok}
          class:error={!sectionMessages["uplink"].ok}
        >
          {sectionMessages["uplink"].text}
        </p>
      {/if}
    </section>

    <!-- Status HTTP -->
    <section class="config-card">
      <h2>Status HTTP</h2>
      <label>
        Bind Address
        <input type="text" bind:value={statusHttpBind} />
      </label>
      <button on:click={saveStatusHttp} disabled={savingSection["status_http"]}>
        {savingSection["status_http"] ? "Saving..." : "Save Status HTTP"}
      </button>
      {#if sectionMessages["status_http"]}
        <p
          class:success={sectionMessages["status_http"].ok}
          class:error={!sectionMessages["status_http"].ok}
        >
          {sectionMessages["status_http"].text}
        </p>
      {/if}
    </section>

    <!-- Readers -->
    <section class="config-card">
      <h2>Readers</h2>
      <table>
        <thead>
          <tr>
            <th>Target *</th>
            <th>Enabled</th>
            <th>Fallback Port</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each readers as reader, i}
            <tr>
              <td>
                <input
                  type="text"
                  bind:value={reader.target}
                  required
                  placeholder="192.168.0.50:10000"
                />
              </td>
              <td>
                <input type="checkbox" bind:checked={reader.enabled} />
              </td>
              <td>
                <input
                  type="number"
                  bind:value={reader.local_fallback_port}
                  min="1"
                  max="65535"
                  placeholder=""
                />
              </td>
              <td>
                <button class="remove-btn" on:click={() => removeReader(i)}>
                  Remove
                </button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="readers-actions">
        <button on:click={addReader}>+ Add Reader</button>
        <button on:click={saveReaders} disabled={savingSection["readers"]}>
          {savingSection["readers"] ? "Saving..." : "Save Readers"}
        </button>
      </div>
      {#if sectionMessages["readers"]}
        <p
          class:success={sectionMessages["readers"].ok}
          class:error={!sectionMessages["readers"].ok}
        >
          {sectionMessages["readers"].text}
        </p>
      {/if}
    </section>
  {/if}
</main>

<style>
  main {
    max-width: 900px;
    margin: 0 auto;
    padding: 1rem;
    font-family: sans-serif;
  }
  a {
    color: #0070f3;
    text-decoration: none;
  }
  a:hover {
    text-decoration: underline;
  }
  h1 {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }
  .restart-banner {
    background: #fff3cd;
    color: #856404;
    border: 1px solid #ffc107;
    border-radius: 4px;
    padding: 0.75rem 1rem;
    margin-bottom: 1rem;
    font-weight: 500;
  }
  .config-card {
    border: 1px solid #ccc;
    border-radius: 4px;
    padding: 1rem;
    margin-bottom: 1rem;
  }
  .config-card h2 {
    margin-top: 0;
    font-size: 1.1rem;
  }
  label {
    display: block;
    margin-bottom: 0.75rem;
    font-size: 0.9em;
    font-weight: 500;
  }
  label input {
    display: block;
    width: 100%;
    margin-top: 0.25rem;
    padding: 0.35rem 0.5rem;
    box-sizing: border-box;
  }
  button {
    padding: 0.35rem 0.85rem;
    cursor: pointer;
    margin-top: 0.25rem;
  }
  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .badge {
    font-size: 0.55em;
    padding: 0.15rem 0.5rem;
    border-radius: 3px;
    font-weight: bold;
    vertical-align: middle;
  }
  .online {
    background: #d4edda;
    color: #155724;
  }
  .offline {
    background: #f8d7da;
    color: #721c24;
  }
  .error {
    color: red;
    font-size: 0.85em;
  }
  .success {
    color: green;
    font-size: 0.85em;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    margin-bottom: 0.5rem;
  }
  th {
    text-align: left;
    padding: 0.35rem 0.5rem;
    border-bottom: 2px solid #ccc;
    font-size: 0.85em;
  }
  td {
    padding: 0.35rem 0.5rem;
    border-bottom: 1px solid #eee;
  }
  td input[type="text"],
  td input[type="number"] {
    width: 100%;
    padding: 0.25rem 0.4rem;
    box-sizing: border-box;
  }
  .readers-actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }
  .remove-btn {
    font-size: 0.8em;
    padding: 0.2rem 0.5rem;
    color: #dc3545;
    border: 1px solid #dc3545;
    background: white;
    border-radius: 3px;
  }
  .remove-btn:hover {
    background: #dc3545;
    color: white;
  }
</style>
