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
  state: string,
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
