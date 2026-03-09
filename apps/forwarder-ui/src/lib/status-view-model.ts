import type { ReaderStatus } from "./api";

export {
  formatReadMode,
  formatTtoState,
  formatClockDrift,
  readerControlDisabled,
  computeDownloadPercent,
  computeTickingLastSeen,
  computeElapsedSecondsSince,
  driftColorClass,
  formatLastSeen,
} from "@rusty-timer/shared-ui/lib/reader-view-model";

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
