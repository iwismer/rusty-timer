export function resolveHeaderBgClass(
  borderStatus: "ok" | "warn" | "err" | undefined,
  headerBg: boolean,
): string {
  if (borderStatus === "ok") return "bg-status-ok-bg";
  if (borderStatus === "warn") return "bg-status-warn-bg";
  if (borderStatus === "err") return "bg-status-err-bg";
  return headerBg ? "bg-surface-2" : "";
}
