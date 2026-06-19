import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  autoIncrement,
  computeBaseOctet,
  readSbcSetupPreference,
  writeSbcSetupPreference,
} from "./persistence";
import type { SbcSetupFormData } from "./types";

const storage = new Map<string, string>();

vi.stubGlobal("localStorage", {
  getItem: vi.fn((key: string) => storage.get(key) ?? null),
  setItem: vi.fn((key: string, value: string) => storage.set(key, value)),
});

const baseForm: SbcSetupFormData = {
  hostname: "rt-fwd-01",
  adminUsername: "rt-admin",
  sshPublicKey: "ssh-ed25519 AAAA test",
  staticIpv4Cidr: "192.168.1.51/24",
  gateway: "192.168.1.1",
  dnsServers: "8.8.8.8,8.8.4.4",
  wifiEnabled: true,
  wifiSsid: "Race WiFi",
  wifiPassword: "wifi-secret",
  wifiCountry: "CA",
  serverUrl: "https://timer.example.com",
  authToken: "token-secret",
  readerTargets: "192.168.1.10:10000",
  statusBind: "0.0.0.0:80",
  displayName: "Start Line",
  setupScriptUrl: "https://example.com/rt-setup.sh",
  upsEnabled: true,
};

describe("sbc setup persistence", () => {
  beforeEach(() => {
    storage.clear();
    vi.clearAllMocks();
  });

  it("writes preferences without auth token or wifi password secrets", () => {
    expect(writeSbcSetupPreference({ form: baseForm, ipBaseOctet: 50 })).toBe(
      true,
    );

    const raw = storage.get("rusty-timer-sbc-setup");
    expect(raw).toBeTruthy();
    expect(raw).not.toContain("token-secret");
    expect(raw).not.toContain("wifi-secret");
  });

  it("reads stored preferences and restores secret fields as blank", () => {
    writeSbcSetupPreference({ form: baseForm, ipBaseOctet: 50 });

    const loaded = readSbcSetupPreference();

    expect(loaded?.form.hostname).toBe("rt-fwd-01");
    expect(loaded?.form.authToken).toBe("");
    expect(loaded?.form.wifiPassword).toBe("");
    expect(loaded?.ipBaseOctet).toBe(50);
  });

  it("computes base octet and auto-increments hostname and IP", () => {
    expect(computeBaseOctet("rt-fwd-01", "192.168.1.51/24")).toBe(50);
    expect(
      autoIncrement({
        hostname: "rt-fwd-01",
        staticIpv4Cidr: "192.168.1.51/24",
        ipBaseOctet: 50,
      }),
    ).toEqual({
      hostname: "rt-fwd-02",
      staticIpv4Cidr: "192.168.1.52/24",
      ipBaseOctet: 50,
    });
  });
});
