import type { ReaderStatus } from "./api";
import type { DownloadProgressEvent } from "./download-progress";

export function formatLastRead(secs: number | null): string {
  if (secs === null) return "never";
  if (secs < 60) return `${secs}s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  return `${Math.floor(secs / 3600)}h ago`;
}

export function readerBadgeState(
  state: ReaderStatus["state"],
): "ok" | "warn" | "err" {
  if (state === "connected") return "ok";
  if (state === "connecting") return "warn";
  return "err";
}

export function readerConnectionSummary(readers: ReaderStatus[]): {
  connected: number;
  configured: number;
  label: string;
} {
  const connected = readers.filter((r) => r.state === "connected").length;
  const configured = readers.length;
  return {
    connected,
    configured,
    label: `${connected} connected / ${configured} configured`,
  };
}

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
  state: ReaderStatus["state"],
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

export function driftColorClass(ms: number | null | undefined): string {
  if (ms == null) return "";
  const abs = Math.abs(ms);
  if (abs < 100) return "text-green-500";
  if (abs < 500) return "text-yellow-500";
  return "text-red-500";
}

export function computeDownloadPercent(
  download: DownloadProgressEvent | null | undefined,
  estimatedReads: number | null | undefined,
): number {
  if (!download) return 0;
  if (download.state !== "downloading")
    return download.state === "complete" ? 100 : 0;

  if (estimatedReads != null && estimatedReads > 0) {
    return Math.min(
      100,
      Math.max(0, Math.round((download.reads_received / estimatedReads) * 100)),
    );
  }

  if (download.total > 0) {
    return Math.min(
      100,
      Math.max(0, Math.round((download.progress / download.total) * 100)),
    );
  }

  return 0;
}

export function computeTickingLastRead(
  baseSecs: number | null,
  receivedAt: number | null,
  now: number,
): number | null {
  if (baseSecs == null) return null;
  if (receivedAt == null) return baseSecs;
  const elapsedSecs = Math.max(0, Math.floor((now - receivedAt) / 1000));
  return baseSecs + elapsedSecs;
}
