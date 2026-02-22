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
  updateMode: string;
  controlAllowPowerActions: boolean;
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
  const update = asRecord(cfg.update);
  const control = asRecord(cfg.control);

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
    updateMode: asString(update.mode),
    controlAllowPowerActions: control.allow_power_actions === true,
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

export function toControlPayload(
  form: ForwarderConfigFormState,
): Record<string, unknown> {
  return { allow_power_actions: form.controlAllowPowerActions };
}

export function toUpdatePayload(
  form: ForwarderConfigFormState,
): Record<string, unknown> {
  return { mode: form.updateMode || null };
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

// --- Validation ---

export function validateGeneral(form: ForwarderConfigFormState): string | null {
  const name = form.generalDisplayName;
  if (!name) return null; // optional field
  if (!name.trim()) return "Display name must not be blank.";
  if (name.includes("\n") || name.includes("\r")) {
    return "Display name must not contain newlines.";
  }
  if (name.length > 150) return "Display name must be 150 characters or fewer.";
  return null;
}

export function validateServer(form: ForwarderConfigFormState): string | null {
  const url = form.serverBaseUrl.trim();
  if (!url) return "Base URL is required.";
  if (!/^https?:\/\/.+/.test(url)) {
    return "Base URL must start with http:// or https://.";
  }
  return null;
}

export function validateAuth(form: ForwarderConfigFormState): string | null {
  const path = form.authTokenFile.trim();
  if (!path) return "Token file path is required.";
  if (path.includes("\n") || path.includes("\r")) {
    return "Token file path must be a single-line path.";
  }
  return null;
}

export function validateJournal(form: ForwarderConfigFormState): string | null {
  if (form.journalPruneWatermarkPct) {
    const pct = Number(form.journalPruneWatermarkPct);
    if (!Number.isFinite(pct) || !Number.isInteger(pct) || pct < 0 || pct > 100) {
      return "Prune watermark must be an integer between 0 and 100.";
    }
  }
  return null;
}

export function validateUplink(form: ForwarderConfigFormState): string | null {
  if (form.uplinkBatchFlushMs) {
    const ms = Number(form.uplinkBatchFlushMs);
    if (!Number.isFinite(ms) || ms < 0 || !Number.isInteger(ms)) {
      return "Batch flush must be a non-negative integer.";
    }
  }
  if (form.uplinkBatchMaxEvents) {
    const max = Number(form.uplinkBatchMaxEvents);
    if (!Number.isFinite(max) || max < 0 || !Number.isInteger(max)) {
      return "Batch max events must be a non-negative integer.";
    }
  }
  return null;
}

export function validateStatusHttp(
  form: ForwarderConfigFormState,
): string | null {
  const bind = form.statusHttpBind.trim();
  if (bind && !isValidIpv4Bind(bind)) {
    return "Bind address must be a valid IPv4 address with port (e.g. 0.0.0.0:8080).";
  }
  return null;
}

function isValidIpv4Bind(bind: string): boolean {
  const match = bind.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3}):(\d{1,5})$/);
  if (!match) return false;

  const octets = match.slice(1, 5).map(Number);
  const port = Number(match[5]);
  if (octets.some((octet) => !Number.isInteger(octet) || octet < 0 || octet > 255)) {
    return false;
  }
  if (!Number.isInteger(port) || port < 0 || port > 65535) {
    return false;
  }
  return true;
}

export function validateReaders(
  form: ForwarderConfigFormState,
): string | null {
  if (form.readers.length === 0) return "At least one reader is required.";
  for (let i = 0; i < form.readers.length; i++) {
    const r = form.readers[i];
    if (!r.target.trim()) return `Reader ${i + 1}: target is required.`;
    if (r.local_fallback_port) {
      const port = Number(r.local_fallback_port);
      if (!Number.isInteger(port) || port < 1 || port > 65535) {
        return `Reader ${i + 1}: fallback port must be between 1 and 65535.`;
      }
    }
  }
  return null;
}

/** Compute the default fallback port from a reader target address. */
export function defaultFallbackPort(target: string): string {
  const match = target.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})(:\d+)?$/);
  if (match) {
    const octets = match.slice(1, 5).map(Number);
    if (octets.every((octet) => Number.isInteger(octet) && octet >= 0 && octet <= 255)) {
      return String(10000 + octets[3]);
    }
  }
  return "";
}
