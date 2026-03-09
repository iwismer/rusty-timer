import { describe, expect, it } from "vitest";
import {
  fromConfig,
  parseTarget,
  buildTarget,
  blankSingleReader,
  blankRangeReader,
  toGeneralPayload,
  toControlPayload,
  toReadersPayload,
  toUpdatePayload,
  validateGeneral,
  validateServer,
  validateAuth,
  validateJournal,
  validateUplink,
  validateStatusHttp,
  validateReaders,
  defaultFallbackPort,
  type ForwarderConfigFormState,
  type ReaderEntry,
  type SingleReaderEntry,
  type RangeReaderEntry,
} from "./forwarder-config-form";

// Helper to build a single-IP reader entry
function makeSingleReader(overrides: Partial<SingleReaderEntry> = {}): SingleReaderEntry {
  return { ...blankSingleReader(), ip: "192.168.0.1", ...overrides };
}

// Helper to build a range reader entry
function makeRangeReader(overrides: Partial<RangeReaderEntry> = {}): RangeReaderEntry {
  return { ...blankRangeReader(), ip_start: "192.168.0.100", ip_end_octet: "110", ...overrides };
}

// Backward-compat alias for tests that only use single-reader fields
const makeReader = makeSingleReader;

function makeForm(overrides: Partial<ForwarderConfigFormState> = {}): ForwarderConfigFormState {
  return {
    generalDisplayName: "",
    serverBaseUrl: "http://localhost:8080",
    serverForwardersWsPath: "",
    authTokenFile: "/tmp/token.txt",
    journalSqlitePath: "",
    journalPruneWatermarkPct: "",
    uplinkBatchMode: "",
    uplinkBatchFlushMs: "",
    uplinkBatchMaxEvents: "",
    statusHttpBind: "",
    controlAllowPowerActions: false,
    updateMode: "",
    readers: [makeReader()],
    ...overrides,
  };
}

describe("parseTarget", () => {
  it("parses a simple IP:port target", () => {
    expect(parseTarget("192.168.0.50:10000")).toEqual({
      is_range: false,
      ip: "192.168.0.50",
      port: "10000",
    });
  });

  it("parses a range target", () => {
    expect(parseTarget("192.168.0.150-160:10000")).toEqual({
      is_range: true,
      ip_start: "192.168.0.150",
      ip_end_octet: "160",
      port: "10000",
    });
  });

  it("returns defaults for empty string", () => {
    expect(parseTarget("")).toEqual({
      is_range: false,
      ip: "",
      port: "10000",
    });
  });

  it("defaults port to 10000 when no colon present", () => {
    expect(parseTarget("192.168.0.50")).toEqual({
      is_range: false,
      ip: "192.168.0.50",
      port: "10000",
    });
  });

  it("parses a range target without port", () => {
    expect(parseTarget("10.0.0.100-200")).toEqual({
      is_range: true,
      ip_start: "10.0.0.100",
      ip_end_octet: "200",
      port: "10000",
    });
  });

  it("handles non-standard port", () => {
    expect(parseTarget("10.0.0.5:9999")).toEqual({
      is_range: false,
      ip: "10.0.0.5",
      port: "9999",
    });
  });

  it("treats trailing dash as single IP (not range)", () => {
    expect(parseTarget("192.168.0.50-:10000")).toEqual({
      is_range: false,
      ip: "192.168.0.50-",
      port: "10000",
    });
  });

  it("treats dash in wrong octet position as single IP", () => {
    expect(parseTarget("192.168.0-50:10000")).toEqual({
      is_range: false,
      ip: "192.168.0-50",
      port: "10000",
    });
  });

  it("treats double-dash as single IP", () => {
    expect(parseTarget("192.168.0.50--160:10000")).toEqual({
      is_range: false,
      ip: "192.168.0.50--160",
      port: "10000",
    });
  });
});

describe("buildTarget", () => {
  it("builds a single-IP target string", () => {
    const reader = makeReader({ ip: "192.168.0.50", port: "10000" });
    expect(buildTarget(reader)).toBe("192.168.0.50:10000");
  });

  it("builds a range target string", () => {
    const reader = makeRangeReader({ ip_start: "192.168.0.150", ip_end_octet: "160", port: "10000" });
    expect(buildTarget(reader)).toBe("192.168.0.150-160:10000");
  });

  it("returns empty string when ip is empty (single mode)", () => {
    const reader = makeReader({ ip: "", port: "10000" });
    expect(buildTarget(reader)).toBe("");
  });

  it("returns empty string when port is empty", () => {
    const reader = makeReader({ ip: "192.168.0.1", port: "" });
    expect(buildTarget(reader)).toBe("");
  });

  it("returns empty string when range start IP is empty", () => {
    const reader = makeRangeReader({ ip_start: "", ip_end_octet: "160", port: "10000" });
    expect(buildTarget(reader)).toBe("");
  });

  it("returns empty string when range end octet is empty", () => {
    const reader = makeRangeReader({ ip_start: "192.168.0.150", ip_end_octet: "", port: "10000" });
    expect(buildTarget(reader)).toBe("");
  });

  it("trims whitespace from fields", () => {
    const reader = makeReader({ ip: " 192.168.0.50 ", port: " 10000 " });
    expect(buildTarget(reader)).toBe("192.168.0.50:10000");
  });

  it("returns empty string when port is NaN (cleared number input)", () => {
    const reader = makeReader({ ip: "192.168.0.1", port: NaN as unknown as string });
    expect(buildTarget(reader)).toBe("");
  });
});

describe("parseTarget/buildTarget round-trip", () => {
  it("round-trips a single-IP target", () => {
    const original = "192.168.0.50:10000";
    const parsed = parseTarget(original);
    expect(parsed.is_range).toBe(false);
    if (!parsed.is_range) {
      const reader = makeSingleReader({ ip: parsed.ip, port: parsed.port });
      expect(buildTarget(reader)).toBe(original);
    }
  });

  it("round-trips a range target", () => {
    const original = "192.168.0.150-160:10000";
    const parsed = parseTarget(original);
    expect(parsed.is_range).toBe(true);
    if (parsed.is_range) {
      const reader = makeRangeReader({ ip_start: parsed.ip_start, ip_end_octet: parsed.ip_end_octet, port: parsed.port });
      expect(buildTarget(reader)).toBe(original);
    }
  });

  it("round-trips a non-standard port", () => {
    const original = "10.0.0.5:9999";
    const parsed = parseTarget(original);
    expect(parsed.is_range).toBe(false);
    if (!parsed.is_range) {
      const reader = makeSingleReader({ ip: parsed.ip, port: parsed.port });
      expect(buildTarget(reader)).toBe(original);
    }
  });
});

describe("fromConfig", () => {
  it("normalizes missing sections to empty form defaults", () => {
    const form = fromConfig({});
    expect(form.serverBaseUrl).toBe("");
    expect(form.controlAllowPowerActions).toBe(false);
    expect(form.readers).toEqual([]);
  });

  it("loads control.allow_power_actions when present", () => {
    const form = fromConfig({
      control: {
        allow_power_actions: true,
      },
    });
    expect(form.controlAllowPowerActions).toBe(true);
  });

  it("reads update.mode when present", () => {
    const form = fromConfig({ update: { mode: "check-only" } });
    expect(form.updateMode).toBe("check-only");
  });

  it("defaults updateMode to empty string when update section missing", () => {
    const form = fromConfig({});
    expect(form.updateMode).toBe("");
  });

  it("parses reader targets into split fields", () => {
    const form = fromConfig({
      readers: [
        { target: "192.168.0.50:10000", enabled: true, local_fallback_port: 10050 },
      ],
    });
    expect(form.readers).toEqual([
      {
        ip: "192.168.0.50",
        port: "10000",
        is_range: false,
        enabled: true,
        local_fallback_port: "10050",
      },
    ]);
  });

  it("parses range reader targets", () => {
    const form = fromConfig({
      readers: [
        { target: "192.168.0.150-160:10000", enabled: false },
      ],
    });
    const r = form.readers[0];
    expect(r.is_range).toBe(true);
    if (r.is_range) {
      expect(r.ip_start).toBe("192.168.0.150");
      expect(r.ip_end_octet).toBe("160");
    }
    expect(r.enabled).toBe(false);
  });

  it("drops persisted local fallback overrides for range readers", () => {
    const form = fromConfig({
      readers: [
        { target: "192.168.0.150-160:10000", enabled: true, local_fallback_port: 12000 },
      ],
    });
    expect(form.readers[0].is_range).toBe(true);
    expect("local_fallback_port" in form.readers[0]).toBe(false);
  });

  it("defaults enabled to true when not specified", () => {
    const form = fromConfig({
      readers: [{ target: "10.0.0.1:10000" }],
    });
    expect(form.readers[0].enabled).toBe(true);
  });

  it("defaults local_fallback_port to empty string when not specified", () => {
    const form = fromConfig({
      readers: [{ target: "10.0.0.1:10000" }],
    });
    const r = form.readers[0];
    expect(r.is_range).toBe(false);
    if (!r.is_range) {
      expect(r.local_fallback_port).toBe("");
    }
  });

  it("handles mixed single and range readers in same config", () => {
    const form = fromConfig({
      readers: [
        { target: "192.168.0.50:10000", enabled: true, local_fallback_port: 10050 },
        { target: "192.168.0.150-160:10000", enabled: false },
        { target: "10.0.0.1:9999", enabled: true },
      ],
    });
    expect(form.readers).toHaveLength(3);
    expect(form.readers[0].is_range).toBe(false);
    expect(form.readers[1].is_range).toBe(true);
    expect(form.readers[2].is_range).toBe(false);
    if (!form.readers[0].is_range) {
      expect(form.readers[0].ip).toBe("192.168.0.50");
      expect(form.readers[0].local_fallback_port).toBe("10050");
    }
    if (form.readers[1].is_range) {
      expect(form.readers[1].ip_start).toBe("192.168.0.150");
      expect(form.readers[1].ip_end_octet).toBe("160");
    }
    if (!form.readers[2].is_range) {
      expect(form.readers[2].ip).toBe("10.0.0.1");
      expect(form.readers[2].port).toBe("9999");
    }
  });
});

describe("payload builders", () => {
  it("serializes empty display name as null", () => {
    expect(
      toGeneralPayload({
        generalDisplayName: "",
      } as ForwarderConfigFormState),
    ).toEqual({
      display_name: null,
    });
  });

  it("serializes readers using buildTarget", () => {
    const form = {
      readers: [
        makeReader({ ip: "127.0.0.1", port: "10001", local_fallback_port: "12484" }),
        makeReader({ ip: "127.0.0.1", port: "10002", enabled: false, local_fallback_port: "" }),
      ],
    } as ForwarderConfigFormState;

    expect(toReadersPayload(form)).toEqual({
      readers: [
        {
          target: "127.0.0.1:10001",
          enabled: true,
          local_fallback_port: 12484,
        },
        {
          target: "127.0.0.1:10002",
          enabled: false,
          local_fallback_port: null,
        },
      ],
    });
  });

  it("serializes range readers using buildTarget", () => {
    const form = {
      readers: [
        makeRangeReader({
          ip_start: "192.168.0.150",
          ip_end_octet: "160",
          port: "10000",
        }),
      ],
    } as ForwarderConfigFormState;

    expect(toReadersPayload(form)).toEqual({
      readers: [
        {
          target: "192.168.0.150-160:10000",
          enabled: true,
          local_fallback_port: null,
        },
      ],
    });
  });

  it("serializes disabled reader with empty IP as null target", () => {
    const form = {
      readers: [makeReader({ ip: "", port: "10000", enabled: false })],
    } as ForwarderConfigFormState;
    const payload = toReadersPayload(form);
    expect(payload.readers[0].target).toBeNull();
  });

  it("serializes disabled reader with empty port as null target", () => {
    const form = {
      readers: [makeReader({ ip: "192.168.0.1", port: "", enabled: false })],
    } as ForwarderConfigFormState;
    const payload = toReadersPayload(form);
    expect(payload.readers[0].target).toBeNull();
  });

  it("serializes control allow_power_actions boolean", () => {
    const form = {
      controlAllowPowerActions: true,
    } as ForwarderConfigFormState;

    expect(toControlPayload(form)).toEqual({
      allow_power_actions: true,
    });
  });

  it("serializes update mode", () => {
    expect(
      toUpdatePayload({
        updateMode: "check-only",
      } as ForwarderConfigFormState),
    ).toEqual({ mode: "check-only" });
  });

  it("serializes empty update mode as null", () => {
    expect(
      toUpdatePayload({
        updateMode: "",
      } as ForwarderConfigFormState),
    ).toEqual({ mode: null });
  });
});

describe("validateGeneral", () => {
  it("passes when empty (optional)", () => {
    expect(validateGeneral(makeForm({ generalDisplayName: "" }))).toBeNull();
  });

  it("passes for a normal name", () => {
    expect(validateGeneral(makeForm({ generalDisplayName: "Start Line" }))).toBeNull();
  });

  it("rejects all-whitespace name", () => {
    expect(validateGeneral(makeForm({ generalDisplayName: "   " }))).toBeTruthy();
  });

  it("rejects name with newline", () => {
    expect(validateGeneral(makeForm({ generalDisplayName: "Start\nLine" }))).toBeTruthy();
  });

  it("rejects name over 150 characters", () => {
    expect(validateGeneral(makeForm({ generalDisplayName: "A".repeat(151) }))).toBeTruthy();
  });

  it("passes for exactly 150 characters", () => {
    expect(validateGeneral(makeForm({ generalDisplayName: "A".repeat(150) }))).toBeNull();
  });
});

describe("validateServer", () => {
  it("passes for valid http URL", () => {
    expect(validateServer(makeForm({ serverBaseUrl: "http://example.com" }))).toBeNull();
  });

  it("passes for valid https URL", () => {
    expect(validateServer(makeForm({ serverBaseUrl: "https://example.com:8443" }))).toBeNull();
  });

  it("rejects empty URL", () => {
    expect(validateServer(makeForm({ serverBaseUrl: "" }))).toBeTruthy();
  });

  it("rejects ws:// URL", () => {
    expect(validateServer(makeForm({ serverBaseUrl: "ws://example.com" }))).toBeTruthy();
  });

  it("rejects URL without scheme", () => {
    expect(validateServer(makeForm({ serverBaseUrl: "example.com" }))).toBeTruthy();
  });
});

describe("validateAuth", () => {
  it("passes for valid path", () => {
    expect(validateAuth(makeForm({ authTokenFile: "/tmp/token.txt" }))).toBeNull();
  });

  it("rejects empty path", () => {
    expect(validateAuth(makeForm({ authTokenFile: "" }))).toBeTruthy();
  });

  it("rejects path with newline", () => {
    expect(validateAuth(makeForm({ authTokenFile: "/tmp/\ntoken.txt" }))).toBeTruthy();
  });
});

describe("validateJournal", () => {
  it("passes when empty (uses default)", () => {
    expect(validateJournal(makeForm({ journalPruneWatermarkPct: "" }))).toBeNull();
  });

  it("passes for valid percentage", () => {
    expect(validateJournal(makeForm({ journalPruneWatermarkPct: "80" }))).toBeNull();
  });

  it("rejects percentage over 100", () => {
    expect(validateJournal(makeForm({ journalPruneWatermarkPct: "101" }))).toBeTruthy();
  });

  it("rejects negative percentage", () => {
    expect(validateJournal(makeForm({ journalPruneWatermarkPct: "-1" }))).toBeTruthy();
  });

  it("rejects non-integer percentage", () => {
    expect(validateJournal(makeForm({ journalPruneWatermarkPct: "80.5" }))).toBeTruthy();
  });
});

describe("validateUplink", () => {
  it("passes when all empty (uses defaults)", () => {
    expect(validateUplink(makeForm())).toBeNull();
  });

  it("passes for valid values", () => {
    expect(validateUplink(makeForm({ uplinkBatchFlushMs: "200", uplinkBatchMaxEvents: "100" }))).toBeNull();
  });

  it("rejects negative batch flush", () => {
    expect(validateUplink(makeForm({ uplinkBatchFlushMs: "-1" }))).toBeTruthy();
  });

  it("rejects non-integer batch max events", () => {
    expect(validateUplink(makeForm({ uplinkBatchMaxEvents: "3.5" }))).toBeTruthy();
  });
});

describe("validateStatusHttp", () => {
  it("passes when empty (uses default)", () => {
    expect(validateStatusHttp(makeForm({ statusHttpBind: "" }))).toBeNull();
  });

  it("passes for valid bind address", () => {
    expect(validateStatusHttp(makeForm({ statusHttpBind: "0.0.0.0:8080" }))).toBeNull();
  });

  it("rejects missing port", () => {
    expect(validateStatusHttp(makeForm({ statusHttpBind: "0.0.0.0" }))).toBeTruthy();
  });

  it("rejects hostname instead of IP", () => {
    expect(validateStatusHttp(makeForm({ statusHttpBind: "localhost:8080" }))).toBeTruthy();
  });

  it("rejects ipv6 bind", () => {
    expect(validateStatusHttp(makeForm({ statusHttpBind: "[::1]:8080" }))).toBeTruthy();
  });

  it("rejects octet out of range", () => {
    expect(validateStatusHttp(makeForm({ statusHttpBind: "999.0.0.1:8080" }))).toBeTruthy();
  });

  it("rejects port out of range", () => {
    expect(validateStatusHttp(makeForm({ statusHttpBind: "127.0.0.1:99999" }))).toBeTruthy();
  });
});

describe("validateReaders", () => {
  it("passes for valid single-IP readers", () => {
    expect(validateReaders(makeForm())).toBeNull();
  });

  it("accepts numeric port values produced by number inputs", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ port: 10000 as unknown as string })],
    }))).toBeNull();
  });

  it("passes for valid range readers", () => {
    expect(validateReaders(makeForm({
      readers: [makeRangeReader()],
    }))).toBeNull();
  });

  it("accepts numeric range end octet values produced by number inputs", () => {
    expect(validateReaders(makeForm({
      readers: [makeRangeReader({
        ip_end_octet: 110 as unknown as string,
      })],
    }))).toBeNull();
  });

  it("rejects empty readers list", () => {
    expect(validateReaders(makeForm({ readers: [] }))).toBeTruthy();
  });

  it("rejects reader with empty IP (single mode)", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ ip: "" })],
    }))).toBeTruthy();
  });

  it("rejects reader with invalid IP (single mode)", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ ip: "999.0.0.1" })],
    }))).toBeTruthy();
  });

  it("rejects reader with non-IP string", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ ip: "reader1.local" })],
    }))).toBeTruthy();
  });

  it("rejects reader with empty port", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ port: "" })],
    }))).toBeTruthy();
  });

  it("rejects reader with port out of range", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ port: "99999" })],
    }))).toBeTruthy();
  });

  it("rejects reader with port 0", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ port: "0" })],
    }))).toBeTruthy();
  });

  it("rejects reader with fallback port out of range", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ local_fallback_port: "99999" })],
    }))).toBeTruthy();
  });

  it("passes when fallback port is empty (optional)", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ local_fallback_port: "" })],
    }))).toBeNull();
  });

  it("rejects range reader with empty start IP", () => {
    expect(validateReaders(makeForm({
      readers: [makeRangeReader({ ip_start: "", ip_end_octet: "110" })],
    }))).toBeTruthy();
  });

  it("rejects range reader with invalid start IP", () => {
    expect(validateReaders(makeForm({
      readers: [makeRangeReader({ ip_start: "999.0.0.100", ip_end_octet: "110" })],
    }))).toBeTruthy();
  });

  it("rejects range reader with empty end octet", () => {
    expect(validateReaders(makeForm({
      readers: [makeRangeReader({ ip_start: "192.168.0.100", ip_end_octet: "" })],
    }))).toBeTruthy();
  });

  it("rejects range reader with end octet > 255", () => {
    expect(validateReaders(makeForm({
      readers: [makeRangeReader({ ip_start: "192.168.0.100", ip_end_octet: "256" })],
    }))).toBeTruthy();
  });

  it("rejects range reader with end octet < start IP last octet", () => {
    expect(validateReaders(makeForm({
      readers: [makeRangeReader({ ip_start: "192.168.0.100", ip_end_octet: "50" })],
    }))).toBeTruthy();
  });

  it("passes range reader with end octet equal to start IP last octet", () => {
    expect(validateReaders(makeForm({
      readers: [makeRangeReader({ ip_start: "192.168.0.100", ip_end_octet: "100" })],
    }))).toBeNull();
  });

  it("rejects reader with whitespace-only IP", () => {
    expect(validateReaders(makeForm({
      readers: [makeSingleReader({ ip: "   " })],
    }))).toBeTruthy();
  });

  it("rejects reader with non-numeric port", () => {
    expect(validateReaders(makeForm({
      readers: [makeSingleReader({ port: "abc" })],
    }))).toBeTruthy();
  });

  it("rejects reader with decimal port", () => {
    expect(validateReaders(makeForm({
      readers: [makeSingleReader({ port: "80.5" })],
    }))).toBeTruthy();
  });

  it("rejects NaN port from cleared number input", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ port: NaN as unknown as string })],
    }))).toBeTruthy();
  });

  it("rejects NaN end octet from cleared number input", () => {
    expect(validateReaders(makeForm({
      readers: [makeRangeReader({ ip_end_octet: NaN as unknown as string })],
    }))).toBeTruthy();
  });

  it("reports correct index for invalid second reader", () => {
    const result = validateReaders(makeForm({
      readers: [makeSingleReader(), makeSingleReader({ ip: "" })],
    }));
    expect(result).toContain("Reader 2");
  });
});

describe("defaultFallbackPort", () => {
  it("computes 10000 + last octet for valid IP", () => {
    expect(defaultFallbackPort("192.168.0.50")).toBe("10050");
  });

  it("computes for another valid IP", () => {
    expect(defaultFallbackPort("10.0.0.1")).toBe("10001");
  });

  it("returns empty for non-IP string", () => {
    expect(defaultFallbackPort("reader1.local")).toBe("");
  });

  it("returns empty for invalid first octet", () => {
    expect(defaultFallbackPort("999.168.0.42")).toBe("");
  });

  it("returns empty for invalid second octet", () => {
    expect(defaultFallbackPort("10.999.0.42")).toBe("");
  });

  it("returns empty for invalid third octet", () => {
    expect(defaultFallbackPort("10.0.999.42")).toBe("");
  });

  it("returns empty for empty string", () => {
    expect(defaultFallbackPort("")).toBe("");
  });

  it("returns empty for IP:port format (not just IP)", () => {
    expect(defaultFallbackPort("192.168.0.50:10000")).toBe("");
  });
});

describe("toReadersPayload", () => {
  it("throws when enabled reader has empty target", () => {
    const form = {
      readers: [makeReader({ ip: "", port: "10000", enabled: true })],
    } as ForwarderConfigFormState;
    expect(() => toReadersPayload(form)).toThrow(/empty target/i);
  });

  it("allows disabled reader with empty target", () => {
    const form = {
      readers: [makeReader({ ip: "", port: "10000", enabled: false })],
    } as ForwarderConfigFormState;
    const payload = toReadersPayload(form);
    expect(payload.readers[0].target).toBeNull();
  });
});
