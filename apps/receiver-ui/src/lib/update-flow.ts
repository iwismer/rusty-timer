import type { UpdateStatusResponse } from "./api";

export type ApplyResult =
  | { outcome: "applied" }
  | { outcome: "failed"; error: string }
  | { outcome: "timeout" };

export interface WaitForApplyOptions {
  attempts?: number;
  intervalMs?: number;
  sleep?: (ms: number) => Promise<void>;
}

function defaultSleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export async function waitForApplyResult(
  getStatus: () => Promise<UpdateStatusResponse>,
  options: WaitForApplyOptions = {},
): Promise<ApplyResult> {
  const attempts = options.attempts ?? 20;
  const intervalMs = options.intervalMs ?? 500;
  const sleep = options.sleep ?? defaultSleep;

  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      const status = await getStatus();
      if (status.status === "failed") {
        return {
          outcome: "failed",
          error: status.error ?? "update apply failed",
        };
      }
      if (status.status === "up_to_date") {
        return { outcome: "applied" };
      }
    } catch {
      // Keep polling through transient request failures.
    }

    if (attempt < attempts - 1) {
      await sleep(intervalMs);
    }
  }

  return { outcome: "timeout" };
}
