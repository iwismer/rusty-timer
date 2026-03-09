export interface ReaderEntry {
  ip: string;
  ip_start: string;
  ip_end_octet: string;
  port: string;
  is_range: boolean;
  enabled: boolean;
  local_fallback_port: string;
}

/** Parse a target string like "192.168.0.50:10000" or "192.168.0.150-160:10000" into split fields. */
export function parseTarget(target: string): Omit<ReaderEntry, "enabled" | "local_fallback_port"> {
  const defaults = { ip: "", ip_start: "", ip_end_octet: "", port: "10000", is_range: false };
  if (!target) return defaults;

  const colonIdx = target.lastIndexOf(":");
  const host = colonIdx >= 0 ? target.slice(0, colonIdx) : target;
  const port = colonIdx >= 0 ? target.slice(colonIdx + 1) : "10000";

  // Check for range syntax: A.B.C.START-END
  const rangeMatch = host.match(/^(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})-(\d{1,3})$/);
  if (rangeMatch) {
    return { ip: "", ip_start: rangeMatch[1], ip_end_octet: rangeMatch[2], port, is_range: true };
  }

  return { ip: host, ip_start: "", ip_end_octet: "", port, is_range: false };
}

/** Build a target string from split fields. */
export function buildTarget(reader: ReaderEntry): string {
  const port = asTrimmedString(reader.port);
  if (reader.is_range) {
    return `${asTrimmedString(reader.ip_start)}-${asTrimmedString(reader.ip_end_octet)}:${port}`;
  }
  return `${asTrimmedString(reader.ip)}:${port}`;
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

function asTrimmedString(value: unknown): string {
  if (typeof value === "string") return value.trim();
  if (typeof value === "number" && Number.isFinite(value)) return String(value);
  return "";
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
    const targetFields = parseTarget(asString(parsed.target));
    return {
      ...targetFields,
      enabled: typeof parsed.enabled === "boolean" ? parsed.enabled : true,
      local_fallback_port:
        !targetFields.is_range && parsed.local_fallback_port != null
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
    readers: form.readers.map((reader) => {
      const fallbackPort = reader.is_range ? "" : asTrimmedString(reader.local_fallback_port);
      return {
        target: buildTarget(reader) || null,
        enabled: reader.enabled,
        local_fallback_port: fallbackPort ? Number(fallbackPort) : null,
      };
    }),
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

function isValidIpv4(ip: string): boolean {
  const match = ip.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/);
  if (!match) return false;
  return match.slice(1, 5).every((o) => {
    const n = Number(o);
    return Number.isInteger(n) && n >= 0 && n <= 255;
  });
}

/** Caller must ensure ip passes isValidIpv4 first. */
function lastOctet(ip: string): number {
  const parts = ip.split(".");
  if (parts.length !== 4) return 0;
  return Number(parts[3]);
}

export function validateReaders(
  form: ForwarderConfigFormState,
): string | null {
  if (form.readers.length === 0) return "At least one reader is required.";
  for (let i = 0; i < form.readers.length; i++) {
    const r = form.readers[i];
    // Port validation (common to both single and range)
    const portText = asTrimmedString(r.port);
    if (!portText) return `Reader ${i + 1}: port is required.`;
    const port = Number(portText);
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      return `Reader ${i + 1}: port must be between 1 and 65535.`;
    }

    if (r.is_range) {
      // Range mode
      const startIp = asTrimmedString(r.ip_start);
      if (!startIp) return `Reader ${i + 1}: start IP is required.`;
      if (!isValidIpv4(startIp)) return `Reader ${i + 1}: start IP must be a valid IPv4 address.`;
      const endOctetText = asTrimmedString(r.ip_end_octet);
      if (!endOctetText) return `Reader ${i + 1}: end octet is required.`;
      const endOctet = Number(endOctetText);
      if (!Number.isInteger(endOctet) || endOctet < 0 || endOctet > 255) {
        return `Reader ${i + 1}: end octet must be between 0 and 255.`;
      }
      if (endOctet < lastOctet(startIp)) {
        return `Reader ${i + 1}: end octet must be >= start IP's last octet.`;
      }
    } else {
      // Single mode
      const ip = asTrimmedString(r.ip);
      if (!ip) return `Reader ${i + 1}: IP is required.`;
      if (!isValidIpv4(ip)) return `Reader ${i + 1}: IP must be a valid IPv4 address.`;
    }

    // Optional fallback port
    const fallbackPortText = asTrimmedString(r.local_fallback_port);
    if (r.is_range && fallbackPortText) {
      return `Reader ${i + 1}: local port override is not supported for ranges.`;
    }
    if (fallbackPortText) {
      const fbPort = Number(fallbackPortText);
      if (!Number.isInteger(fbPort) || fbPort < 1 || fbPort > 65535) {
        return `Reader ${i + 1}: fallback port must be between 1 and 65535.`;
      }
    }
  }
  return null;
}

/** Compute the default fallback port from a reader IP address. */
export function defaultFallbackPort(ip: string): string {
  if (!ip) return "";
  if (!isValidIpv4(ip)) return "";
  return String(10000 + lastOctet(ip));
}
