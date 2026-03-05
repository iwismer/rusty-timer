import { describe, it, expect } from "vitest";
import {
  validateHostname,
  validateUsername,
  validateSshKey,
  validateIpv4Cidr,
  validateIpv4Address,
  parseDnsServers,
  validateBaseUrl,
  validateReaderTarget,
  parseReaderTargets,
  validateStatusBind,
  validateWifiCountry,
} from "./validation";

describe("validateHostname", () => {
  it("accepts valid hostnames", () => {
    expect(validateHostname("rt-fwd-01")).toBe("rt-fwd-01");
    expect(validateHostname("abc")).toBe("abc");
    expect(validateHostname("a1-b2")).toBe("a1-b2");
  });
  it("trims whitespace", () => {
    expect(validateHostname("  rt-fwd-01  ")).toBe("rt-fwd-01");
  });
  it("rejects empty", () => {
    expect(validateHostname("")).toBeInstanceOf(Error);
  });
  it("rejects uppercase", () => {
    expect(validateHostname("RT-FWD-01")).toBeInstanceOf(Error);
  });
  it("rejects leading hyphen", () => {
    expect(validateHostname("-bad")).toBeInstanceOf(Error);
  });
});

describe("validateUsername", () => {
  it("accepts valid usernames", () => {
    expect(validateUsername("rt-admin")).toBe("rt-admin");
    expect(validateUsername("_user1")).toBe("_user1");
  });
  it("rejects empty", () => {
    expect(validateUsername("")).toBeInstanceOf(Error);
  });
  it("rejects starting with number", () => {
    expect(validateUsername("1bad")).toBeInstanceOf(Error);
  });
});

describe("validateSshKey", () => {
  it("accepts keys starting with ssh-", () => {
    expect(validateSshKey("ssh-ed25519 AAAA...")).toBe("ssh-ed25519 AAAA...");
  });
  it("accepts ecdsa-sha2- keys", () => {
    expect(validateSshKey("ecdsa-sha2-nistp256 AAAA...")).toBe(
      "ecdsa-sha2-nistp256 AAAA...",
    );
  });
  it("accepts sk-ssh- keys", () => {
    expect(validateSshKey("sk-ssh-ed25519 AAAA...")).toBe(
      "sk-ssh-ed25519 AAAA...",
    );
  });
  it("accepts sk-ecdsa- keys", () => {
    expect(validateSshKey("sk-ecdsa-sha2-nistp256 AAAA...")).toBe(
      "sk-ecdsa-sha2-nistp256 AAAA...",
    );
  });
  it("rejects empty", () => {
    expect(validateSshKey("")).toBeInstanceOf(Error);
  });
  it("rejects invalid prefix", () => {
    expect(validateSshKey("rsa-bad AAAA...")).toBeInstanceOf(Error);
  });
});

describe("validateIpv4Cidr", () => {
  it("accepts valid CIDR", () => {
    expect(validateIpv4Cidr("192.168.1.50/24")).toBe("192.168.1.50/24");
  });
  it("rejects missing prefix length", () => {
    expect(validateIpv4Cidr("192.168.1.50")).toBeInstanceOf(Error);
  });
  it("rejects invalid octets", () => {
    expect(validateIpv4Cidr("999.999.999.999/24")).toBeInstanceOf(Error);
  });
  it("rejects prefix length > 32", () => {
    expect(validateIpv4Cidr("192.168.1.50/33")).toBeInstanceOf(Error);
  });
  it("rejects empty", () => {
    expect(validateIpv4Cidr("")).toBeInstanceOf(Error);
  });
});

describe("validateIpv4Address", () => {
  it("accepts valid address", () => {
    expect(validateIpv4Address("192.168.1.1")).toBe("192.168.1.1");
  });
  it("rejects invalid", () => {
    expect(validateIpv4Address("not-an-ip")).toBeInstanceOf(Error);
  });
  it("rejects empty", () => {
    expect(validateIpv4Address("")).toBeInstanceOf(Error);
  });
});

describe("parseDnsServers", () => {
  it("parses comma-separated IPs", () => {
    expect(parseDnsServers("8.8.8.8,8.8.4.4")).toEqual(["8.8.8.8", "8.8.4.4"]);
  });
  it("trims whitespace", () => {
    expect(parseDnsServers(" 8.8.8.8 , 8.8.4.4 ")).toEqual([
      "8.8.8.8",
      "8.8.4.4",
    ]);
  });
  it("rejects empty", () => {
    expect(parseDnsServers("")).toBeInstanceOf(Error);
  });
  it("rejects invalid IP in list", () => {
    expect(parseDnsServers("8.8.8.8,bad")).toBeInstanceOf(Error);
  });
});

describe("validateBaseUrl", () => {
  it("accepts http URL and canonicalizes", () => {
    expect(validateBaseUrl("http://example.com")).toBe("http://example.com/");
  });
  it("accepts https URL and canonicalizes", () => {
    expect(validateBaseUrl("https://example.com")).toBe("https://example.com/");
  });
  it("rejects empty", () => {
    expect(validateBaseUrl("")).toBeInstanceOf(Error);
  });
  it("rejects non-http", () => {
    expect(validateBaseUrl("ftp://example.com")).toBeInstanceOf(Error);
  });
  it("rejects protocol-only URL without host", () => {
    expect(validateBaseUrl("https://")).toBeInstanceOf(Error);
  });
});

describe("validateReaderTarget", () => {
  it("accepts IP:PORT", () => {
    expect(validateReaderTarget("192.168.1.10:10000")).toBe(
      "192.168.1.10:10000",
    );
  });
  it("accepts IP range:PORT", () => {
    expect(validateReaderTarget("192.168.1.150-160:10000")).toBe(
      "192.168.1.150-160:10000",
    );
  });
  it("rejects missing port", () => {
    expect(validateReaderTarget("192.168.1.10")).toBeInstanceOf(Error);
  });
  it("rejects invalid IP octets", () => {
    expect(validateReaderTarget("999.999.999.999:99999")).toBeInstanceOf(Error);
  });
  it("rejects port 0", () => {
    expect(validateReaderTarget("192.168.1.10:0")).toBeInstanceOf(Error);
  });
});

describe("parseReaderTargets", () => {
  it("splits by newline", () => {
    expect(
      parseReaderTargets("192.168.1.10:10000\n192.168.1.11:10000"),
    ).toEqual(["192.168.1.10:10000", "192.168.1.11:10000"]);
  });
  it("rejects empty", () => {
    expect(parseReaderTargets("")).toBeInstanceOf(Error);
  });
});

describe("validateStatusBind", () => {
  it("accepts valid bind address", () => {
    expect(validateStatusBind("0.0.0.0:80")).toBe("0.0.0.0:80");
  });
  it("accepts localhost with port", () => {
    expect(validateStatusBind("127.0.0.1:8080")).toBe("127.0.0.1:8080");
  });
  it("rejects missing port", () => {
    expect(validateStatusBind("badaddr")).toBeInstanceOf(Error);
  });
  it("rejects invalid IP", () => {
    expect(validateStatusBind("999.0.0.1:80")).toBeInstanceOf(Error);
  });
  it("rejects port 0", () => {
    expect(validateStatusBind("0.0.0.0:0")).toBeInstanceOf(Error);
  });
  it("rejects empty", () => {
    expect(validateStatusBind("")).toBeInstanceOf(Error);
  });
});

describe("validateWifiCountry", () => {
  it("accepts 2-letter code", () => {
    expect(validateWifiCountry("US")).toBe("US");
  });
  it("uppercases input", () => {
    expect(validateWifiCountry("ca")).toBe("CA");
  });
  it("rejects 3-letter code", () => {
    expect(validateWifiCountry("USA")).toBeInstanceOf(Error);
  });
});
