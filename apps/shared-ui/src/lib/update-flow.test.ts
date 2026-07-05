import { describe, it, expect, vi } from "vitest";
import { waitForApplyResult } from "./update-flow";
import type { UpdateStatusResponse } from "./update-flow";

const noSleep = async () => {};

describe("waitForApplyResult", () => {
  it("returns failed when status becomes failed", async () => {
    const getStatus = vi
      .fn<[], Promise<UpdateStatusResponse>>()
      .mockResolvedValueOnce({ status: "downloaded", version: "1.2.3" })
      .mockResolvedValueOnce({ status: "failed", error: "boom" });

    await expect(
      waitForApplyResult(getStatus, {
        attempts: 2,
        intervalMs: 1,
        sleep: noSleep,
      }),
    ).resolves.toEqual({ outcome: "failed", error: "boom" });
  });

  it("returns applied when status becomes up_to_date", async () => {
    const getStatus = vi
      .fn<[], Promise<UpdateStatusResponse>>()
      .mockResolvedValueOnce({ status: "downloaded", version: "1.2.3" })
      .mockResolvedValueOnce({ status: "up_to_date" });

    await expect(
      waitForApplyResult(getStatus, {
        attempts: 2,
        intervalMs: 1,
        sleep: noSleep,
      }),
    ).resolves.toEqual({ outcome: "applied" });
  });

  it("returns timeout while status remains non-terminal", async () => {
    const getStatus = vi
      .fn<[], Promise<UpdateStatusResponse>>()
      .mockResolvedValueOnce({ status: "downloaded", version: "1.2.3" })
      .mockResolvedValueOnce({ status: "available", version: "1.2.3" });

    await expect(
      waitForApplyResult(getStatus, {
        attempts: 2,
        intervalMs: 1,
        sleep: noSleep,
      }),
    ).resolves.toEqual({ outcome: "timeout" });
  });

  it("retries after transient status fetch errors", async () => {
    const getStatus = vi
      .fn<[], Promise<UpdateStatusResponse>>()
      .mockRejectedValueOnce(new Error("network"))
      .mockResolvedValueOnce({ status: "up_to_date" });

    await expect(
      waitForApplyResult(getStatus, {
        attempts: 2,
        intervalMs: 1,
        sleep: noSleep,
      }),
    ).resolves.toEqual({ outcome: "applied" });
  });
});
