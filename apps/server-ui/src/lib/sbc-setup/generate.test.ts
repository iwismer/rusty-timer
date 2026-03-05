// generate.test.ts
import { describe, it, expect } from "vitest";
import { generateUserData, generateNetworkConfig } from "./generate";
import type { SbcSetupFormData } from "./types";

function baseConfig(): SbcSetupFormData {
  return {
    hostname: "rt-fwd-01",
    adminUsername: "rt-admin",
    sshPublicKey: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5 user@host",
    staticIpv4Cidr: "192.168.1.50/24",
    gateway: "192.168.1.1",
    dnsServers: "8.8.8.8,8.8.4.4",
    wifiEnabled: false,
    wifiSsid: "",
    wifiPassword: "",
    wifiCountry: "US",
    serverBaseUrl: "https://timer.example.com",
    authToken: "tok_abc123",
    readerTargets: "192.168.1.10:10000",
    statusBind: "0.0.0.0:80",
    displayName: "rt-fwd-01",
  };
}

describe("generateUserData", () => {
  it("includes cloud-config header", () => {
    const result = generateUserData(baseConfig());
    expect(result).toMatch(/^#cloud-config\n/);
  });

  it("includes hostname", () => {
    const result = generateUserData(baseConfig());
    expect(result).toContain("hostname: 'rt-fwd-01'");
  });

  it("includes SSH key in single quotes", () => {
    const result = generateUserData(baseConfig());
    expect(result).toContain("'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5 user@host'");
  });

  it("includes admin username", () => {
    const result = generateUserData(baseConfig());
    expect(result).toContain("name: 'rt-admin'");
  });

  it("includes rt-forwarder system user", () => {
    const result = generateUserData(baseConfig());
    expect(result).toContain("name: rt-forwarder");
  });

  it("includes auto-first-boot packages", () => {
    const result = generateUserData(baseConfig());
    expect(result).toContain("curl");
    expect(result).toContain("tar");
  });

  it("includes setup env file with server URL", () => {
    const result = generateUserData(baseConfig());
    expect(result).toContain(
      "RT_SETUP_SERVER_BASE_URL='https://timer.example.com'",
    );
  });

  it("includes setup env file with auth token", () => {
    const result = generateUserData(baseConfig());
    expect(result).toContain("RT_SETUP_AUTH_TOKEN='tok_abc123'");
  });

  it("includes runcmd to download and run rt-setup.sh", () => {
    const result = generateUserData(baseConfig());
    expect(result).toContain("curl -fsSL");
    expect(result).toContain("rt-setup.sh");
  });

  it("shell-escapes auth token with single quote", () => {
    const config = { ...baseConfig(), authToken: "tok_it's" };
    const result = generateUserData(config);
    expect(result).toContain("RT_SETUP_AUTH_TOKEN='tok_it'\\''s'");
  });

  it("shell-escapes display name with single quote", () => {
    const config = { ...baseConfig(), displayName: "O'Brien" };
    const result = generateUserData(config);
    expect(result).toContain("RT_SETUP_DISPLAY_NAME='O'\\''Brien'");
  });

  it("falls back to hostname when displayName is empty", () => {
    const config = { ...baseConfig(), displayName: "" };
    const result = generateUserData(config);
    expect(result).toContain("RT_SETUP_DISPLAY_NAME='rt-fwd-01'");
  });

  it("filters blank lines from multi-line reader targets", () => {
    const config = {
      ...baseConfig(),
      readerTargets: "192.168.1.10:10000\n\n192.168.1.11:10000\n",
    };
    const result = generateUserData(config);
    expect(result).toContain(
      "RT_SETUP_READER_TARGETS='192.168.1.10:10000,192.168.1.11:10000'",
    );
  });
});

describe("generateNetworkConfig", () => {
  it("includes network version 2 header", () => {
    const result = generateNetworkConfig(baseConfig());
    expect(result).toContain("version: 2");
  });

  it("includes static IP on eth0", () => {
    const result = generateNetworkConfig(baseConfig());
    expect(result).toContain("'192.168.1.50/24'");
  });

  it("includes gateway", () => {
    const result = generateNetworkConfig(baseConfig());
    expect(result).toContain("via: '192.168.1.1'");
  });

  it("includes DNS servers", () => {
    const result = generateNetworkConfig(baseConfig());
    expect(result).toContain("- '8.8.8.8'");
    expect(result).toContain("- '8.8.4.4'");
  });

  it("does not include wifi when disabled", () => {
    const result = generateNetworkConfig(baseConfig());
    expect(result).not.toContain("wifis:");
  });

  it("includes wifi when enabled", () => {
    const config = {
      ...baseConfig(),
      wifiEnabled: true,
      wifiSsid: "MyNet",
      wifiPassword: "secret",
      wifiCountry: "US",
    };
    const result = generateNetworkConfig(config);
    expect(result).toContain("wifis:");
    expect(result).toContain("'MyNet'");
    expect(result).toContain("password: 'secret'");
    expect(result).toContain("regulatory-domain: 'US'");
  });

  it("handles open wifi (no password)", () => {
    const config = {
      ...baseConfig(),
      wifiEnabled: true,
      wifiSsid: "OpenNet",
      wifiPassword: "",
      wifiCountry: "CA",
    };
    const result = generateNetworkConfig(config);
    expect(result).toContain("'OpenNet': {}");
    expect(result).not.toContain("password:");
  });

  it("YAML-escapes wifi SSID with single quote", () => {
    const config = {
      ...baseConfig(),
      wifiEnabled: true,
      wifiSsid: "Bob's Net",
      wifiPassword: "pass",
      wifiCountry: "US",
    };
    const result = generateNetworkConfig(config);
    expect(result).toContain("'Bob''s Net':");
  });

  it("YAML-escapes wifi password with single quote", () => {
    const config = {
      ...baseConfig(),
      wifiEnabled: true,
      wifiSsid: "MyNet",
      wifiPassword: "it's-secret",
      wifiCountry: "US",
    };
    const result = generateNetworkConfig(config);
    expect(result).toContain("password: 'it''s-secret'");
  });
});
