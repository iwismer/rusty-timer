import { describe, it, expect } from "vitest";
import { createLatestRequestGate } from "./latestRequestGate";

describe("createLatestRequestGate", () => {
  it("accepts only the most recent request token", () => {
    const gate = createLatestRequestGate();

    const first = gate.next();
    const second = gate.next();

    expect(gate.isLatest(first)).toBe(false);
    expect(gate.isLatest(second)).toBe(true);
  });

  it("invalidates in-flight requests when selection is cleared", () => {
    const gate = createLatestRequestGate();

    const inFlight = gate.next();
    gate.invalidate();

    expect(gate.isLatest(inFlight)).toBe(false);
  });

  it("drops stale response token after stream switch", () => {
    const gate = createLatestRequestGate();

    const streamA = gate.next();
    const streamB = gate.next();

    expect(gate.isLatest(streamA)).toBe(false);
    expect(gate.isLatest(streamB)).toBe(true);
  });
});
