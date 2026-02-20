export interface ReaderEntry {
  target: string;
  enabled: boolean;
  local_fallback_port: string;
}

export interface ForwarderConfigFormState {
  generalDisplayName: string;
  serverBaseUrl: string;
  serverForwardersWsPath: string;
  authTokenFile: string;
  journalSqlitePath: string;
  journalPruneWatermarkPct: string;
  uplinkBatchMode: string;
  uplinkBatchFlushMs: string;
  uplinkBatchMaxEvents: string;
  statusHttpBind: string;
  readers: ReaderEntry[];
}

function asRecord(value: unknown): Record<string, unknown> {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value as Record<string, unknown>;
  }
  return {};
}

function asString(value: unknown): string {
  return typeof value === "string" ? value : "";
}

export function fromConfig(cfg: Record<string, unknown>): ForwarderConfigFormState {
  const server = asRecord(cfg.server);
  const auth = asRecord(cfg.auth);
  const journal = asRecord(cfg.journal);
  const uplink = asRecord(cfg.uplink);
  const statusHttp = asRecord(cfg.status_http);

  const rawReaders = Array.isArray(cfg.readers) ? cfg.readers : [];
  const readers: ReaderEntry[] = rawReaders.map((reader) => {
    const parsed = asRecord(reader);
    return {
      target: asString(parsed.target),
      enabled: typeof parsed.enabled === "boolean" ? parsed.enabled : true,
      local_fallback_port:
        parsed.local_fallback_port != null
          ? String(parsed.local_fallback_port)
          : "",
    };
  });

  return {
    generalDisplayName: asString(cfg.display_name),
    serverBaseUrl: asString(server.base_url),
    serverForwardersWsPath: asString(server.forwarders_ws_path),
    authTokenFile: asString(auth.token_file),
    journalSqlitePath: asString(journal.sqlite_path),
    journalPruneWatermarkPct:
      journal.prune_watermark_pct != null
        ? String(journal.prune_watermark_pct)
        : "",
    uplinkBatchMode: asString(uplink.batch_mode),
    uplinkBatchFlushMs:
      uplink.batch_flush_ms != null ? String(uplink.batch_flush_ms) : "",
    uplinkBatchMaxEvents:
      uplink.batch_max_events != null ? String(uplink.batch_max_events) : "",
    statusHttpBind: asString(statusHttp.bind),
    readers,
  };
}

export function toGeneralPayload(
  form: ForwarderConfigFormState,
): Record<string, unknown> {
  return { display_name: form.generalDisplayName || null };
}

export function toServerPayload(
  form: ForwarderConfigFormState,
): Record<string, unknown> {
  return {
    base_url: form.serverBaseUrl,
    forwarders_ws_path: form.serverForwardersWsPath || null,
  };
}

export function toAuthPayload(
  form: ForwarderConfigFormState,
): Record<string, unknown> {
  return { token_file: form.authTokenFile };
}

export function toJournalPayload(
  form: ForwarderConfigFormState,
): Record<string, unknown> {
  return {
    sqlite_path: form.journalSqlitePath || null,
    prune_watermark_pct: form.journalPruneWatermarkPct
      ? Number(form.journalPruneWatermarkPct)
      : null,
  };
}

export function toUplinkPayload(
  form: ForwarderConfigFormState,
): Record<string, unknown> {
  return {
    batch_mode: form.uplinkBatchMode || null,
    batch_flush_ms: form.uplinkBatchFlushMs
      ? Number(form.uplinkBatchFlushMs)
      : null,
    batch_max_events: form.uplinkBatchMaxEvents
      ? Number(form.uplinkBatchMaxEvents)
      : null,
  };
}

export function toStatusHttpPayload(
  form: ForwarderConfigFormState,
): Record<string, unknown> {
  return { bind: form.statusHttpBind || null };
}

export function toReadersPayload(
  form: ForwarderConfigFormState,
): Record<string, unknown> {
  return {
    readers: form.readers.map((reader) => ({
      target: reader.target || null,
      enabled: reader.enabled,
      local_fallback_port: reader.local_fallback_port
        ? Number(reader.local_fallback_port)
        : null,
    })),
  };
}
