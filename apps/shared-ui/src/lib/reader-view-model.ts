export function formatReadMode(mode: string | null | undefined): string {
  if (mode == null) return "\u2014";
  if (mode === "fsls") return "FS/LS";
  if (mode === "raw") return "Raw";
  if (mode === "event") return "Event";
  return mode;
}

export function formatTtoState(enabled: boolean | null | undefined): string {
  if (enabled == null) return "\u2014";
  return enabled ? "Enabled" : "Disabled";
}

export function formatEpochCreatedAt(
  unixMs: number | null | undefined,
  locale?: string,
  timeZone?: string,
): string {
  if (unixMs == null) return "\u2014";
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
    timeZone,
  }).format(new Date(unixMs));
}

export function formatEpochName(name: string | null | undefined): string {
  return normalizeEpochNameDraft(name ?? "") ?? "unnamed";
}

export function normalizeEpochNameDraft(draft: string): string | null {
  return draft.trim() || null;
}

export async function advanceEpochWithOptionalName(
  draftName: string,
  onAdvanceEpoch: () => Promise<void>,
  onSetEpochName?: (name: string) => Promise<void>,
): Promise<"advanced" | "advanced_and_named"> {
  const name = normalizeEpochNameDraft(draftName);
  await onAdvanceEpoch();
  if (name !== null && onSetEpochName) {
    await onSetEpochName(name);
    return "advanced_and_named";
  }
  return "advanced";
}

export function readerControlDisabled(
  state: "connected" | "connecting" | "disconnected",
  busy: boolean | null | undefined,
): boolean {
  return Boolean(busy) || state !== "connected";
}

export function formatClockDrift(ms: number | null | undefined): string {
  if (ms == null) return "\u2014";
  const abs = Math.abs(ms);
  const sign = ms >= 0 ? "+" : "-";
  if (abs < 1000) return `${sign}${abs}ms`;
  return `${sign}${(abs / 1000).toFixed(1)}s`;
}

export function computeDownloadPercent(
  download:
    | {
        state: string;
        reads_received?: number;
        progress?: number;
        total?: number;
      }
    | null
    | undefined,
  estimatedReads: number | null | undefined,
): number {
  if (!download) return 0;
  if (download.state !== "downloading")
    return download.state === "complete" ? 100 : 0;

  if (
    estimatedReads != null &&
    estimatedReads > 0 &&
    download.reads_received != null
  ) {
    return Math.min(
      100,
      Math.max(
        0,
        Math.round((download.reads_received / estimatedReads) * 100),
      ),
    );
  }

  if (download.total != null && download.total > 0 && download.progress != null) {
    return Math.min(
      100,
      Math.max(0, Math.round((download.progress / download.total) * 100)),
    );
  }

  return 0;
}

export function computeTickingLastSeen(
  baseSecs: number | null,
  receivedAt: number | null,
  now: number,
): number | null {
  if (baseSecs == null) return null;
  if (receivedAt == null) return baseSecs;
  const elapsedSecs = Math.max(0, Math.floor((now - receivedAt) / 1000));
  return baseSecs + elapsedSecs;
}

export function computeElapsedSecondsSince(
  receivedAt: number,
  now: number,
): number {
  return Math.max(0, Math.round((now - receivedAt) / 1000));
}

/**
 * Format a millisecond timestamp as `YYYY-MM-DD HH:MM:SS` using its UTC fields.
 *
 * Reader/forwarder wall clocks are naive (zoneless) values that we anchor to a
 * consistent UTC instant, so both clocks must be rendered with the same UTC
 * fields to stay in the same displayed timezone.
 */
export function formatWallClock(ms: number): string {
  const d = new Date(ms);
  const y = d.getUTCFullYear();
  const mo = String(d.getUTCMonth() + 1).padStart(2, "0");
  const day = String(d.getUTCDate()).padStart(2, "0");
  const h = String(d.getUTCHours()).padStart(2, "0");
  const mi = String(d.getUTCMinutes()).padStart(2, "0");
  const s = String(d.getUTCSeconds()).padStart(2, "0");
  return `${y}-${mo}-${day} ${h}:${mi}:${s}`;
}

/**
 * Parse a reader/forwarder wall-clock string into a millisecond timestamp,
 * anchoring the naive (zoneless) value to UTC so it round-trips with
 * {@link formatWallClock}. Accepts `YYYY-MM-DD HH:MM:SS[.mmm]` and ISO forms.
 * Returns `NaN` for unparseable input.
 */
export function parseWallClock(iso: string): number {
  const normalized = iso.replace(" ", "T");
  const withZ = normalized.endsWith("Z") ? normalized : normalized + "Z";
  return new Date(withZ).getTime();
}

/**
 * Advance a clock captured at `baseTs` (a naive wall time anchored to UTC) by
 * the real time elapsed since it was captured (`now - baseLocal`), then render
 * it. `offsetMs` shifts the result — used to derive the forwarder clock from the
 * reader clock plus the measured reader/forwarder drift. Returns `null` when the
 * base is unavailable so callers can render a placeholder.
 */
export function computeTickingClock(
  baseTs: number | null | undefined,
  baseLocal: number | null | undefined,
  now: number,
  offsetMs = 0,
): string | null {
  if (baseTs == null || baseLocal == null) return null;
  const elapsed = now - baseLocal;
  return formatWallClock(baseTs + offsetMs + elapsed);
}

export function driftColorClass(ms: number | null | undefined): string {
  if (ms == null) return "";
  const abs = Math.abs(ms);
  if (abs < 100) return "text-green-500";
  if (abs < 500) return "text-yellow-500";
  return "text-red-500";
}

/**
 * Format a reader hardware code showing both hex and decimal forms, e.g.
 * `0x45 (69)`. Accepts numbers, decimal strings, or `0x`-prefixed hex
 * strings. Falls back to the raw string when unparseable and `\u2014` for
 * null/undefined/empty input.
 */
export function formatHardwareCode(
  code: string | number | null | undefined,
): string {
  if (code == null) return "\u2014";
  let value: number;
  if (typeof code === "number") {
    value = code;
  } else {
    const trimmed = code.trim();
    if (trimmed === "") return "\u2014";
    if (/^0x[0-9a-f]+$/i.test(trimmed)) {
      value = parseInt(trimmed, 16);
    } else if (/^[0-9]+$/.test(trimmed)) {
      value = parseInt(trimmed, 10);
    } else {
      return code;
    }
  }
  if (!Number.isInteger(value) || value < 0) return String(code);
  return `0x${value.toString(16)} (${value})`;
}

export function formatLastSeen(secs: number | null): string {
  if (secs === null) return "never";
  if (secs < 60) return `${secs}s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  return `${Math.floor(secs / 3600)}h ago`;
}
