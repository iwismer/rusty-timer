<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "$lib/api";
  import type { ForwarderConfig } from "$lib/api";

  let config: ForwarderConfig | null = null;
  let loadError: string | null = null;
  let sectionMessages: Record<string, { ok: boolean; text: string }> = {};
  let restartNeeded = false;

  // Form state (populated from loaded config)
  let displayName = "";
  let baseUrl = "";
  let wsPath = "";
  let tokenFile = "";
  let sqlitePath = "";
  let pruneWatermarkPct: number | null = null;
  let batchMode = "";
  let batchFlushMs: number | null = null;
  let batchMaxEvents: number | null = null;
  let bind = "";
  let readers: Array<{
    target: string;
    enabled: boolean;
    local_fallback_port: number | null;
  }> = [];

  onMount(async () => {
    try {
      config = await api.getConfig();
      displayName = config.display_name ?? "";
      baseUrl = config.server?.base_url ?? "";
      wsPath = config.server?.forwarders_ws_path ?? "";
      tokenFile = config.auth?.token_file ?? "";
      sqlitePath = config.journal?.sqlite_path ?? "";
      pruneWatermarkPct = config.journal?.prune_watermark_pct ?? null;
      batchMode = config.uplink?.batch_mode ?? "";
      batchFlushMs = config.uplink?.batch_flush_ms ?? null;
      batchMaxEvents = config.uplink?.batch_max_events ?? null;
      bind = config.status_http?.bind ?? "";
      readers = (config.readers ?? []).map((r) => ({
        target: r.target ?? "",
        enabled: r.enabled ?? true,
        local_fallback_port: r.local_fallback_port ?? null,
      }));
    } catch (e) {
      loadError = String(e);
    }
  });

  async function saveSection(
    section: string,
    data: Record<string, unknown>,
  ) {
    sectionMessages = { ...sectionMessages, [section]: undefined as any };
    try {
      const result = await api.saveConfigSection(section, data);
      if (result.ok) {
        sectionMessages = {
          ...sectionMessages,
          [section]: { ok: true, text: "Saved. Restart to apply." },
        };
        restartNeeded = true;
      } else {
        sectionMessages = {
          ...sectionMessages,
          [section]: { ok: false, text: result.error ?? "Unknown error" },
        };
      }
    } catch (e) {
      sectionMessages = {
        ...sectionMessages,
        [section]: { ok: false, text: String(e) },
      };
    }
  }

  function saveGeneral() {
    saveSection("general", {
      display_name: displayName || null,
    });
  }

  function saveServer() {
    saveSection("server", {
      base_url: baseUrl || null,
      forwarders_ws_path: wsPath || null,
    });
  }

  function saveAuth() {
    saveSection("auth", {
      token_file: tokenFile || null,
    });
  }

  function saveJournal() {
    saveSection("journal", {
      sqlite_path: sqlitePath || null,
      prune_watermark_pct: pruneWatermarkPct,
    });
  }

  function saveUplink() {
    saveSection("uplink", {
      batch_mode: batchMode || null,
      batch_flush_ms: batchFlushMs,
      batch_max_events: batchMaxEvents,
    });
  }

  function saveStatusHttp() {
    saveSection("status_http", {
      bind: bind || null,
    });
  }

  function saveReaders() {
    saveSection("readers", {
      readers: readers.map((r) => ({
        target: r.target || null,
        enabled: r.enabled,
        local_fallback_port: r.local_fallback_port,
      })),
    });
  }

  function addReader() {
    readers = [
      ...readers,
      { target: "", enabled: true, local_fallback_port: null },
    ];
  }

  function removeReader(index: number) {
    readers = readers.filter((_, i) => i !== index);
  }

  async function handleRestart() {
    try {
      await api.restart();
    } catch (e) {
      loadError = String(e);
    }
  }
</script>

<main>
  <h1>Forwarder Configuration</h1>

  {#if restartNeeded}
    <div class="restart-banner">
      Configuration changed. Restart to apply.
      <button on:click={handleRestart}>Restart Now</button>
    </div>
  {/if}

  {#if loadError}
    <p class="error">{loadError}</p>
  {/if}

  {#if config}
    <section>
      <h2>General</h2>
      <label>
        Display Name
        <input type="text" bind:value={displayName} />
      </label>
      <button class="save" on:click={saveGeneral}>Save General</button>
      {#if sectionMessages.general}
        <p class={sectionMessages.general.ok ? "msg-ok" : "msg-err"}>
          {sectionMessages.general.text}
        </p>
      {/if}
    </section>

    <section>
      <h2>Server</h2>
      <label>
        Base URL *
        <input type="text" bind:value={baseUrl} required />
      </label>
      <label>
        Forwarders WS Path
        <input type="text" bind:value={wsPath} />
      </label>
      <button class="save" on:click={saveServer}>Save Server</button>
      {#if sectionMessages.server}
        <p class={sectionMessages.server.ok ? "msg-ok" : "msg-err"}>
          {sectionMessages.server.text}
        </p>
      {/if}
    </section>

    <section>
      <h2>Auth</h2>
      <label>
        Token File Path *
        <input type="text" bind:value={tokenFile} required />
      </label>
      <button class="save" on:click={saveAuth}>Save Auth</button>
      {#if sectionMessages.auth}
        <p class={sectionMessages.auth.ok ? "msg-ok" : "msg-err"}>
          {sectionMessages.auth.text}
        </p>
      {/if}
    </section>

    <section>
      <h2>Journal</h2>
      <label>
        SQLite Path
        <input type="text" bind:value={sqlitePath} />
      </label>
      <label>
        Prune Watermark %
        <input
          type="number"
          bind:value={pruneWatermarkPct}
          min="0"
          max="100"
        />
      </label>
      <button class="save" on:click={saveJournal}>Save Journal</button>
      {#if sectionMessages.journal}
        <p class={sectionMessages.journal.ok ? "msg-ok" : "msg-err"}>
          {sectionMessages.journal.text}
        </p>
      {/if}
    </section>

    <section>
      <h2>Uplink</h2>
      <label>
        Batch Mode
        <input type="text" bind:value={batchMode} />
      </label>
      <label>
        Batch Flush (ms)
        <input type="number" bind:value={batchFlushMs} min="1" />
      </label>
      <label>
        Batch Max Events
        <input type="number" bind:value={batchMaxEvents} min="1" />
      </label>
      <button class="save" on:click={saveUplink}>Save Uplink</button>
      {#if sectionMessages.uplink}
        <p class={sectionMessages.uplink.ok ? "msg-ok" : "msg-err"}>
          {sectionMessages.uplink.text}
        </p>
      {/if}
    </section>

    <section>
      <h2>Status HTTP</h2>
      <label>
        Bind Address
        <input type="text" bind:value={bind} />
      </label>
      <button class="save" on:click={saveStatusHttp}>Save Status HTTP</button>
      {#if sectionMessages.status_http}
        <p class={sectionMessages.status_http.ok ? "msg-ok" : "msg-err"}>
          {sectionMessages.status_http.text}
        </p>
      {/if}
    </section>

    <section>
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
                <input type="text" bind:value={reader.target} required />
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
                />
              </td>
              <td>
                <button on:click={() => removeReader(i)}>Remove</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="reader-actions">
        <button on:click={addReader}>+ Add Reader</button>
        <button class="save" on:click={saveReaders}>Save Readers</button>
      </div>
      {#if sectionMessages.readers}
        <p class={sectionMessages.readers.ok ? "msg-ok" : "msg-err"}>
          {sectionMessages.readers.text}
        </p>
      {/if}
    </section>
  {:else if !loadError}
    <p>Loading configuration...</p>
  {/if}
</main>

<style>
  table {
    border-collapse: collapse;
    width: 100%;
    margin-bottom: 0.5rem;
  }
  th, td {
    text-align: left;
    padding: 0.3rem 0.4rem;
    border-bottom: 1px solid #ddd;
  }
  th {
    font-weight: 600;
    font-size: 0.9rem;
  }
  td input[type="text"],
  td input[type="number"] {
    width: 100%;
  }
  td input[type="checkbox"] {
    width: auto;
  }
  .save {
    background: var(--color-ok-bg);
    border-color: var(--color-ok);
    color: var(--color-ok);
    margin-top: 0.5rem;
  }
  .save:hover {
    background: #c3e6cb;
  }
  .msg-ok {
    background: var(--color-ok-bg);
    color: var(--color-ok);
    padding: 0.5rem;
    border-radius: 4px;
    margin-top: 0.5rem;
  }
  .msg-err {
    background: var(--color-err-bg);
    color: var(--color-err);
    padding: 0.5rem;
    border-radius: 4px;
    margin-top: 0.5rem;
  }
  .error {
    color: var(--color-err);
  }
  .restart-banner {
    background: var(--color-warn-bg);
    color: var(--color-warn);
    border: 1px solid #ffc107;
    padding: 0.75rem 1rem;
    border-radius: 4px;
    margin-bottom: 1rem;
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .reader-actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }
</style>
