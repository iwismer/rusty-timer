import { describe, expect, it } from "vitest";
import {
  parseLogLevel,
  parseLogEntry,
  countByLevel,
  filterEntries,
  filterEntriesAdvanced,
} from "./log-filter";

describe("parseLogLevel", () => {
  it("extracts DEBUG from tagged entry", () => {
    expect(parseLogLevel("12:34:56 [DEBUG] sent batch")).toBe("debug");
  });

  it("extracts WARN from tagged entry with milliseconds", () => {
    expect(parseLogLevel("12:34:56.123 [WARN] connection lost")).toBe("warn");
  });

  it("extracts INFO from tagged entry", () => {
    expect(parseLogLevel("12:34:56 [INFO] server started")).toBe("info");
  });

  it("extracts ERROR from tagged entry with milliseconds", () => {
    expect(parseLogLevel("12:34:56.999 [ERROR] crash")).toBe("error");
  });

  it("extracts TRACE from tagged entry", () => {
    expect(parseLogLevel("12:34:56 [TRACE] detail")).toBe("trace");
  });

  it("returns info for untagged entry", () => {
    expect(parseLogLevel("12:34:56 some old message")).toBe("info");
  });

  it("returns info for unknown tag", () => {
    expect(parseLogLevel("12:34:56 [UNKNOWN] msg")).toBe("info");
  });

  it("does not match bracket text in the middle of a message", () => {
    expect(parseLogLevel("12:34:56 [INFO] value is [DEBUG] ok")).toBe("info");
  });
});

describe("parseLogEntry", () => {
  it("parses entry with component source tag", () => {
    const parsed = parseLogEntry("12:34:56.789 [WARN] [SERVER:WS] forwarder fwd-1 disconnected: timeout");
    expect(parsed.timestamp).toBe("12:34:56.789");
    expect(parsed.level).toBe("warn");
    expect(parsed.sourceTag).toBe("SERVER:WS");
    expect(parsed.message).toBe("forwarder fwd-1 disconnected: timeout");
  });

  it("parses entry without component tag", () => {
    const parsed = parseLogEntry("12:34:56 [INFO] server listening on 0.0.0.0:8080");
    expect(parsed.timestamp).toBe("12:34:56");
    expect(parsed.level).toBe("info");
    expect(parsed.sourceTag).toBeUndefined();
    expect(parsed.message).toBe("server listening on 0.0.0.0:8080");
  });

  it("handles untagged plain text entries gracefully", () => {
    const parsed = parseLogEntry("12:34:56 old plain message");
    expect(parsed.timestamp).toBe("12:34:56");
    expect(parsed.level).toBe("info");
    expect(parsed.message).toBe("old plain message");
  });
});

describe("countByLevel", () => {
  it("counts entries by level correctly", () => {
    const entries = [
      "12:00:00 [DEBUG] debug msg",
      "12:00:01 [INFO] info msg 1",
      "12:00:02 [INFO] info msg 2",
      "12:00:03 [WARN] warn msg",
      "12:00:04 [ERROR] error msg",
      "12:00:05 untagged",
    ];
    const counts = countByLevel(entries);
    expect(counts.debug).toBe(1);
    expect(counts.info).toBe(3); // includes untagged
    expect(counts.warn).toBe(1);
    expect(counts.error).toBe(1);
    expect(counts.trace).toBe(0);
  });
});

describe("filterEntries", () => {
  const entries = [
    "12:00:00 [DEBUG] batch sent",
    "12:00:01 [INFO] connected",
    "12:00:02 [WARN] timeout",
    "12:00:03 [ERROR] crash",
    "12:00:04 old untagged entry",
  ];

  it("at info level, excludes debug but keeps untagged", () => {
    const result = filterEntries(entries, "info");
    expect(result).toHaveLength(4);
    expect(result[0]).toContain("[INFO]");
    expect(result[3]).toContain("old untagged entry");
  });

  it("at debug level, includes all", () => {
    expect(filterEntries(entries, "debug")).toHaveLength(5);
  });

  it("at warn level, excludes debug, info, and untagged", () => {
    const result = filterEntries(entries, "warn");
    expect(result).toHaveLength(2);
    expect(result[0]).toContain("[WARN]");
    expect(result[1]).toContain("[ERROR]");
  });

  it("at error level, includes only error", () => {
    const result = filterEntries(entries, "error");
    expect(result).toHaveLength(1);
    expect(result[0]).toContain("[ERROR]");
  });

  it("at trace level, includes everything", () => {
    expect(filterEntries(entries, "trace")).toHaveLength(5);
  });

  it("returns empty for empty input", () => {
    expect(filterEntries([], "info")).toHaveLength(0);
  });
});

describe("filterEntriesAdvanced", () => {
  const entries = [
    "12:00:00.100 [DEBUG] [FWD:READER] ping reader 192.168.1.10",
    "12:00:01.200 [INFO] [SERVER:WS] forwarder fwd-1 connected",
    "12:00:02.300 [WARN] [SERVER:WS] forwarder fwd-1 timeout",
    "12:00:03.400 [ERROR] [SERVER:DB] pool connection error",
    "12:00:04.500 [INFO] [SERVER:INGEST] stream 1 batch: 10 events",
  ];

  it("filters by selected active levels (warn + info)", () => {
    const result = filterEntriesAdvanced(entries, new Set(["warn", "info"]));
    expect(result).toHaveLength(3);
    expect(result[0]).toContain("[INFO] [SERVER:WS]");
    expect(result[1]).toContain("[WARN] [SERVER:WS]");
    expect(result[2]).toContain("[INFO] [SERVER:INGEST]");
  });

  it("filters by search query case-insensitively", () => {
    const result = filterEntriesAdvanced(entries, new Set(["info", "warn", "error"]), "fwd-1");
    expect(result).toHaveLength(2);
    expect(result[0]).toContain("connected");
    expect(result[1]).toContain("timeout");
  });

  it("combines level filtering and search query", () => {
    const result = filterEntriesAdvanced(entries, new Set(["warn"]), "fwd-1");
    expect(result).toHaveLength(1);
    expect(result[0]).toContain("[WARN]");
  });
});
