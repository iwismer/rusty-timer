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

export function formatLastSeen(secs: number | null): string {
  if (secs === null) return "never";
  if (secs < 60) return `${secs}s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  return `${Math.floor(secs / 3600)}h ago`;
}
