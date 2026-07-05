import { describe, expect, it } from "vitest";
import { parseLogLevel, filterEntries } from "./log-filter";

describe("parseLogLevel", () => {
  it("extracts DEBUG from tagged entry", () => {
    expect(parseLogLevel("12:34:56 [DEBUG] sent batch")).toBe("debug");
  });

  it("extracts WARN from tagged entry", () => {
    expect(parseLogLevel("12:34:56 [WARN] connection lost")).toBe("warn");
  });

  it("extracts INFO from tagged entry", () => {
    expect(parseLogLevel("12:34:56 [INFO] server started")).toBe("info");
  });

  it("extracts ERROR from tagged entry", () => {
    expect(parseLogLevel("12:34:56 [ERROR] crash")).toBe("error");
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
