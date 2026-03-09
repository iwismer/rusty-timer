import { describe, expect, it } from "vitest";
import {
  fromConfig,
  parseTarget,
  buildTarget,
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
} from "./forwarder-config-form";

// Helper to build a single-IP reader entry
function makeReader(overrides: Partial<ReaderEntry> = {}): ReaderEntry {
  return {
    ip: "192.168.0.1",
    ip_start: "",
    ip_end_octet: "",
    port: "10000",
    is_range: false,
    enabled: true,
    local_fallback_port: "",
    ...overrides,
  };
}

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
      ip: "192.168.0.50",
      ip_start: "",
      ip_end_octet: "",
      port: "10000",
      is_range: false,
    });
  });

  it("parses a range target", () => {
    expect(parseTarget("192.168.0.150-160:10000")).toEqual({
      ip: "",
      ip_start: "192.168.0.150",
      ip_end_octet: "160",
      port: "10000",
      is_range: true,
    });
  });

  it("returns defaults for empty string", () => {
    expect(parseTarget("")).toEqual({
      ip: "",
      ip_start: "",
      ip_end_octet: "",
      port: "10000",
      is_range: false,
    });
  });

  it("defaults port to 10000 when no colon present", () => {
    expect(parseTarget("192.168.0.50")).toEqual({
      ip: "192.168.0.50",
      ip_start: "",
      ip_end_octet: "",
      port: "10000",
      is_range: false,
    });
  });

  it("parses a range target without port", () => {
    expect(parseTarget("10.0.0.100-200")).toEqual({
      ip: "",
      ip_start: "10.0.0.100",
      ip_end_octet: "200",
      port: "10000",
      is_range: true,
    });
  });

  it("handles non-standard port", () => {
    expect(parseTarget("10.0.0.5:9999")).toEqual({
      ip: "10.0.0.5",
      ip_start: "",
      ip_end_octet: "",
      port: "9999",
      is_range: false,
    });
  });
});

describe("buildTarget", () => {
  it("builds a single-IP target string", () => {
    const reader = makeReader({ ip: "192.168.0.50", port: "10000" });
    expect(buildTarget(reader)).toBe("192.168.0.50:10000");
  });

  it("builds a range target string", () => {
    const reader = makeReader({
      ip: "",
      ip_start: "192.168.0.150",
      ip_end_octet: "160",
      port: "10000",
      is_range: true,
    });
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
    const reader = makeReader({ ip: "", ip_start: "", ip_end_octet: "160", port: "10000", is_range: true });
    expect(buildTarget(reader)).toBe("");
  });

  it("returns empty string when range end octet is empty", () => {
    const reader = makeReader({ ip: "", ip_start: "192.168.0.150", ip_end_octet: "", port: "10000", is_range: true });
    expect(buildTarget(reader)).toBe("");
  });

  it("trims whitespace from fields", () => {
    const reader = makeReader({ ip: " 192.168.0.50 ", port: " 10000 " });
    expect(buildTarget(reader)).toBe("192.168.0.50:10000");
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
        ip_start: "",
        ip_end_octet: "",
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
    expect(form.readers[0].is_range).toBe(true);
    expect(form.readers[0].ip_start).toBe("192.168.0.150");
    expect(form.readers[0].ip_end_octet).toBe("160");
    expect(form.readers[0].enabled).toBe(false);
  });

  it("drops persisted local fallback overrides for range readers", () => {
    const form = fromConfig({
      readers: [
        { target: "192.168.0.150-160:10000", enabled: true, local_fallback_port: 12000 },
      ],
    });
    expect(form.readers[0].local_fallback_port).toBe("");
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
    expect(form.readers[0].local_fallback_port).toBe("");
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
        makeReader({
          ip: "",
          ip_start: "192.168.0.150",
          ip_end_octet: "160",
          port: "10000",
          is_range: true,
          local_fallback_port: "",
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

  it("serializes range readers without local port overrides", () => {
    const form = {
      readers: [
        makeReader({
          ip: "",
          ip_start: "192.168.0.150",
          ip_end_octet: "160",
          port: "10000",
          is_range: true,
          local_fallback_port: "12000",
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
      readers: [makeReader({
        ip: "",
        ip_start: "192.168.0.100",
        ip_end_octet: "110",
        port: "10000",
        is_range: true,
      })],
    }))).toBeNull();
  });

  it("accepts numeric range end octet values produced by number inputs", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({
        ip: "",
        ip_start: "192.168.0.100",
        ip_end_octet: 110 as unknown as string,
        port: "10000",
        is_range: true,
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

  it("rejects local fallback port overrides for range readers", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({
        ip: "",
        ip_start: "192.168.0.100",
        ip_end_octet: "110",
        port: "10000",
        is_range: true,
        local_fallback_port: "12000",
      })],
    }))).toBe("Reader 1: local port override is not supported for ranges.");
  });

  it("rejects range reader with empty start IP", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ ip: "", ip_start: "", ip_end_octet: "110", is_range: true })],
    }))).toBeTruthy();
  });

  it("rejects range reader with invalid start IP", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ ip: "", ip_start: "999.0.0.100", ip_end_octet: "110", is_range: true })],
    }))).toBeTruthy();
  });

  it("rejects range reader with empty end octet", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ ip: "", ip_start: "192.168.0.100", ip_end_octet: "", is_range: true })],
    }))).toBeTruthy();
  });

  it("rejects range reader with end octet > 255", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ ip: "", ip_start: "192.168.0.100", ip_end_octet: "256", is_range: true })],
    }))).toBeTruthy();
  });

  it("rejects range reader with end octet < start IP last octet", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ ip: "", ip_start: "192.168.0.100", ip_end_octet: "50", is_range: true })],
    }))).toBeTruthy();
  });

  it("passes range reader with end octet equal to start IP last octet", () => {
    expect(validateReaders(makeForm({
      readers: [makeReader({ ip: "", ip_start: "192.168.0.100", ip_end_octet: "100", is_range: true })],
    }))).toBeNull();
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
