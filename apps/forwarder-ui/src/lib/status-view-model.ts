import type { ReaderStatus } from "./api";

export function formatLastSeen(secs: number | null): string {
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

export function formatClockDrift(ms: number | null | undefined): string {
  if (ms == null) return "\u2014";
  const abs = Math.abs(ms);
  const sign = ms >= 0 ? "+" : "-";
  if (abs < 1000) return `${sign}${abs}ms`;
  return `${sign}${(abs / 1000).toFixed(1)}s`;
}
