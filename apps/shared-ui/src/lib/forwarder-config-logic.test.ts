import { describe, expect, it } from "vitest";
import {
  controlPowerActionsEnabled,
  saveSuccessMessage,
} from "./forwarder-config-logic";

describe("saveSuccessMessage", () => {
  it("returns restart copy when restart is required", () => {
    expect(saveSuccessMessage(true)).toBe("Saved. Restart to apply.");
  });

  it("returns generic success copy when restart is not required", () => {
    expect(saveSuccessMessage(false)).toBe("Saved.");
  });
});

describe("controlPowerActionsEnabled", () => {
  it("returns true when persisted and current control settings both allow power actions", () => {
    expect(
      controlPowerActionsEnabled({
        persistedAllowPowerActions: true,
        currentAllowPowerActions: true,
      }),
    ).toBe(true);
  });

  it("returns false when current is enabled but not yet persisted", () => {
    expect(
      controlPowerActionsEnabled({
        persistedAllowPowerActions: false,
        currentAllowPowerActions: true,
      }),
    ).toBe(false);
  });

  it("returns false when persisted is enabled but current setting was toggled off unsaved", () => {
    expect(
      controlPowerActionsEnabled({
        persistedAllowPowerActions: true,
        currentAllowPowerActions: false,
      }),
    ).toBe(false);
  });

  it("returns false when both persisted and current settings disallow power actions", () => {
    expect(
      controlPowerActionsEnabled({
        persistedAllowPowerActions: false,
        currentAllowPowerActions: false,
      }),
    ).toBe(false);
  });
});
