const DEFAULT_TIMEOUT_SECONDS = 5;
const MIN_TIMEOUT_SECONDS = 1;
const MAX_TIMEOUT_SECONDS = 255;

export const READ_MODE_OPTIONS = [
  { value: "raw", label: "Raw" },
  { value: "event", label: "Event" },
  { value: "fsls", label: "First/Last Seen" },
] as const;

export function shouldShowTimeoutInput(
  mode: string | null | undefined,
): boolean {
  return mode === "fsls";
}

export function initialTimeoutDraft(
  current: number | null | undefined,
): string {
  if (typeof current === "number" && Number.isFinite(current)) {
    return String(current);
  }
  return String(DEFAULT_TIMEOUT_SECONDS);
}

export function resolveTimeoutSeconds(
  draft: string,
  fallback: number | null | undefined,
): number {
  const trimmed = draft.trim();
  const parsed = Number.parseInt(trimmed, 10);

  if (Number.isFinite(parsed)) {
    return clampTimeout(parsed);
  }

  if (typeof fallback === "number" && Number.isFinite(fallback)) {
    return clampTimeout(fallback);
  }

  return DEFAULT_TIMEOUT_SECONDS;
}

function clampTimeout(value: number): number {
  return Math.min(MAX_TIMEOUT_SECONDS, Math.max(MIN_TIMEOUT_SECONDS, value));
}
