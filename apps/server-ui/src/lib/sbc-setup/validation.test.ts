import { describe, expect, it } from "vitest";
import {
  parseDnsServers,
  parseReaderTargets,
  validateBaseUrl,
  validateHostname,
  validateIpv4Address,
  validateIpv4Cidr,
  validateSshKey,
  validateStatusBind,
  validateUsername,
  validateWifiCountry,
} from "./validation";

describe("sbc setup validation", () => {
  it("validates hostnames", () => {
    expect(validateHostname("rt-fwd-01")).toBe("rt-fwd-01");
    expect(validateHostname("RT-FWD-01")).toBeInstanceOf(Error);
    expect(validateHostname("")).toBeInstanceOf(Error);
  });

  it("validates usernames", () => {
    expect(validateUsername("rt-admin")).toBe("rt-admin");
    expect(validateUsername("_admin")).toBe("_admin");
    expect(validateUsername("RT Admin")).toBeInstanceOf(Error);
  });

  it("validates ssh keys using the current Python-compatible ssh prefix", () => {
    expect(validateSshKey("ssh-ed25519 AAAA test")).toBe(
      "ssh-ed25519 AAAA test",
    );
    expect(validateSshKey("ecdsa-sha2-nistp256 AAAA test")).toBeInstanceOf(
      Error,
    );
  });

  it("validates ipv4 cidr and addresses", () => {
    expect(validateIpv4Cidr("192.168.1.50/24")).toBe("192.168.1.50/24");
    expect(validateIpv4Cidr("192.168.1.999/24")).toBeInstanceOf(Error);
    expect(validateIpv4Cidr("192.168.1.50/33")).toBeInstanceOf(Error);
    expect(validateIpv4Address("192.168.1.1")).toBe("192.168.1.1");
    expect(validateIpv4Address("not-an-ip")).toBeInstanceOf(Error);
  });

  it("parses dns servers", () => {
    expect(parseDnsServers("1.1.1.1, 8.8.8.8")).toEqual(["1.1.1.1", "8.8.8.8"]);
    expect(parseDnsServers("8.8.8.8,not-an-ip")).toBeInstanceOf(Error);
  });

  it("validates server urls", () => {
    expect(validateBaseUrl("https://timer.example.com")).toBe(
      "https://timer.example.com",
    );
    expect(validateBaseUrl("ftp://timer.example.com")).toBeInstanceOf(Error);
  });

  it("parses reader targets separated by newline comma or semicolon", () => {
    expect(
      parseReaderTargets(
        "192.168.1.10:10000\n192.168.1.150-160:10000;192.168.1.11:10000",
      ),
    ).toEqual([
      "192.168.1.10:10000",
      "192.168.1.150-160:10000",
      "192.168.1.11:10000",
    ]);
    expect(parseReaderTargets("192.168.1.10:70000")).toBeInstanceOf(Error);
  });

  it("validates status bind and wifi country", () => {
    expect(validateStatusBind("0.0.0.0:80")).toBe("0.0.0.0:80");
    expect(validateStatusBind("0.0.0.0:70000")).toBeInstanceOf(Error);
    expect(validateWifiCountry("ca")).toBe("CA");
    expect(validateWifiCountry("can")).toBeInstanceOf(Error);
  });
});
