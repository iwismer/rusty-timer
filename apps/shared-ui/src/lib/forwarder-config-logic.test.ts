import { describe, expect, it } from "vitest";
import { saveSuccessMessage } from "./forwarder-config-logic";

describe("saveSuccessMessage", () => {
  it("returns restart copy when restart is required", () => {
    expect(saveSuccessMessage(true)).toBe("Saved. Restart to apply.");
  });

  it("returns generic success copy when restart is not required", () => {
    expect(saveSuccessMessage(false)).toBe("Saved.");
  });
});
