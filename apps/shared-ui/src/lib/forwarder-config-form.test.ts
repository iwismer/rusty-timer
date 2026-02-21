import { describe, expect, it } from "vitest";
import {
  fromConfig,
  toGeneralPayload,
  toControlPayload,
  toReadersPayload,
  validateGeneral,
  validateServer,
  validateAuth,
  validateJournal,
  validateUplink,
  validateStatusHttp,
  validateReaders,
  defaultFallbackPort,
  type ForwarderConfigFormState,
} from "./forwarder-config-form";

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

  it("serializes readers fallback port as number/null", () => {
    const form = {
      readers: [
        {
          target: "127.0.0.1:10001",
          enabled: true,
          local_fallback_port: "12484",
        },
        {
          target: "127.0.0.1:10002",
          enabled: false,
          local_fallback_port: "",
        },
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

  it("serializes control allow_power_actions boolean", () => {
    const form = {
      controlAllowPowerActions: true,
    } as ForwarderConfigFormState;

    expect(toControlPayload(form)).toEqual({
      allow_power_actions: true,
    });
  });
});

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
    readers: [{ target: "192.168.0.1:10000", enabled: true, local_fallback_port: "" }],
    ...overrides,
  };
}

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
  it("passes for valid readers", () => {
    expect(validateReaders(makeForm())).toBeNull();
  });

  it("rejects empty readers list", () => {
    expect(validateReaders(makeForm({ readers: [] }))).toBeTruthy();
  });

  it("rejects reader with empty target", () => {
    expect(validateReaders(makeForm({
      readers: [{ target: "", enabled: true, local_fallback_port: "" }],
    }))).toBeTruthy();
  });

  it("rejects reader with port out of range", () => {
    expect(validateReaders(makeForm({
      readers: [{ target: "192.168.0.1:10000", enabled: true, local_fallback_port: "99999" }],
    }))).toBeTruthy();
  });
});

describe("defaultFallbackPort", () => {
  it("computes 10000 + last octet for valid IP:port", () => {
    expect(defaultFallbackPort("192.168.0.50:10000")).toBe("10050");
  });

  it("computes for IP without port", () => {
    expect(defaultFallbackPort("10.0.0.1")).toBe("10001");
  });

  it("returns empty for non-IP target", () => {
    expect(defaultFallbackPort("reader1.local:10000")).toBe("");
  });

  it("returns empty for invalid first octet", () => {
    expect(defaultFallbackPort("999.168.0.42:10000")).toBe("");
  });

  it("returns empty for invalid second octet", () => {
    expect(defaultFallbackPort("10.999.0.42:10000")).toBe("");
  });

  it("returns empty for invalid third octet", () => {
    expect(defaultFallbackPort("10.0.999.42:10000")).toBe("");
  });

  it("returns empty for empty string", () => {
    expect(defaultFallbackPort("")).toBe("");
  });
});
