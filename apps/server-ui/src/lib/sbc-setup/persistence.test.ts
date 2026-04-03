import { describe, it, expect, beforeEach, vi } from "vitest";
import {
  readSbcSetupPreference,
  writeSbcSetupPreference,
  autoIncrement,
  computeBaseOctet,
  type SbcSetupStored,
} from "./persistence";

describe("readSbcSetupPreference", () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
  });

  it("returns null when nothing stored", () => {
    vi.stubGlobal("localStorage", {
      getItem: vi.fn().mockReturnValue(null),
    } as unknown as Storage);
    expect(readSbcSetupPreference()).toBe(null);
  });

  it("returns parsed data when stored", () => {
    const stored: SbcSetupStored = {
      hostname: "rt-fwd-02",
      adminUsername: "rt-admin",
      sshPublicKey: "ssh-ed25519 AAAA",
      staticIpv4Cidr: "192.168.1.52/24",
      gateway: "192.168.1.1",
      dnsServers: "8.8.8.8,8.8.4.4",
      wifiEnabled: false,
      wifiSsid: "",
      wifiPassword: "",
      wifiCountry: "US",
      serverBaseUrl: "https://timer.example.com",
      authToken: "tok_123",
      readerTargets: "192.168.1.10:10000",
      statusBind: "0.0.0.0:80",
      displayName: "rt-fwd-02",
      setupScriptUrl:
        "https://raw.githubusercontent.com/iwismer/rusty-timer/master/deploy/sbc/rt-setup.sh",
      upsEnabled: false,
      ipBaseOctet: 50,
    };
    vi.stubGlobal("localStorage", {
      getItem: vi.fn().mockReturnValue(JSON.stringify(stored)),
    } as unknown as Storage);
    expect(readSbcSetupPreference()).toEqual(stored);
  });

  it("returns null on corrupt JSON", () => {
    vi.stubGlobal("localStorage", {
      getItem: vi.fn().mockReturnValue("not-json"),
    } as unknown as Storage);
    expect(readSbcSetupPreference()).toBe(null);
  });

  it("returns null when localStorage is unavailable", () => {
    vi.stubGlobal("localStorage", undefined);
    expect(readSbcSetupPreference()).toBe(null);
  });
});

describe("writeSbcSetupPreference", () => {
  beforeEach(() => {
    vi.unstubAllGlobals();
  });

  it("calls setItem with serialized data", () => {
    const setItem = vi.fn();
    vi.stubGlobal("localStorage", { setItem } as unknown as Storage);
    const data: SbcSetupStored = {
      hostname: "rt-fwd-01",
      adminUsername: "rt-admin",
      sshPublicKey: "ssh-ed25519 AAAA",
      staticIpv4Cidr: "192.168.1.51/24",
      gateway: "192.168.1.1",
      dnsServers: "8.8.8.8",
      wifiEnabled: false,
      wifiSsid: "",
      wifiPassword: "",
      wifiCountry: "US",
      serverBaseUrl: "",
      authToken: "",
      readerTargets: "",
      statusBind: "0.0.0.0:80",
      displayName: "",
      setupScriptUrl:
        "https://raw.githubusercontent.com/iwismer/rusty-timer/master/deploy/sbc/rt-setup.sh",
      upsEnabled: false,
      ipBaseOctet: 50,
    };
    const result = writeSbcSetupPreference(data);
    expect(setItem).toHaveBeenCalledWith("sbcSetup", JSON.stringify(data));
    expect(result).toBe(true);
  });

  it("returns false when localStorage throws", () => {
    vi.stubGlobal("localStorage", {
      setItem: vi.fn().mockImplementation(() => {
        throw new Error("quota exceeded");
      }),
    } as unknown as Storage);
    const data: SbcSetupStored = {
      hostname: "rt-fwd-01",
      adminUsername: "rt-admin",
      sshPublicKey: "",
      staticIpv4Cidr: "192.168.1.51/24",
      gateway: "192.168.1.1",
      dnsServers: "8.8.8.8",
      wifiEnabled: false,
      wifiSsid: "",
      wifiPassword: "",
      wifiCountry: "US",
      serverBaseUrl: "",
      authToken: "",
      readerTargets: "",
      statusBind: "0.0.0.0:80",
      displayName: "",
      setupScriptUrl:
        "https://raw.githubusercontent.com/iwismer/rusty-timer/master/deploy/sbc/rt-setup.sh",
      upsEnabled: false,
      ipBaseOctet: 50,
    };
    const result = writeSbcSetupPreference(data);
    expect(result).toBe(false);
  });
});

describe("autoIncrement", () => {
  it("increments hostname number and IP last octet", () => {
    const result = autoIncrement({
      hostname: "rt-fwd-01",
      staticIpv4Cidr: "192.168.1.51/24",
      ipBaseOctet: 50,
    });
    expect(result.hostname).toBe("rt-fwd-02");
    expect(result.staticIpv4Cidr).toBe("192.168.1.52/24");
    expect(result.ipBaseOctet).toBe(50);
  });

  it("increments from 09 to 10", () => {
    const result = autoIncrement({
      hostname: "rt-fwd-09",
      staticIpv4Cidr: "192.168.1.59/24",
      ipBaseOctet: 50,
    });
    expect(result.hostname).toBe("rt-fwd-10");
    expect(result.staticIpv4Cidr).toBe("192.168.1.60/24");
  });

  it("returns unchanged if hostname has no trailing number", () => {
    const result = autoIncrement({
      hostname: "my-sbc",
      staticIpv4Cidr: "192.168.1.50/24",
      ipBaseOctet: 50,
    });
    expect(result.hostname).toBe("my-sbc");
    expect(result.staticIpv4Cidr).toBe("192.168.1.50/24");
  });

  it("computes base octet from first pair", () => {
    const result = autoIncrement({
      hostname: "rt-fwd-03",
      staticIpv4Cidr: "192.168.1.53/24",
      ipBaseOctet: 50,
    });
    expect(result.hostname).toBe("rt-fwd-04");
    expect(result.staticIpv4Cidr).toBe("192.168.1.54/24");
  });

  it("returns unchanged when next octet would exceed 255", () => {
    const input = {
      hostname: "rt-fwd-205",
      staticIpv4Cidr: "192.168.1.255/24",
      ipBaseOctet: 50,
    };
    const result = autoIncrement(input);
    expect(result.hostname).toBe("rt-fwd-205");
    expect(result.staticIpv4Cidr).toBe("192.168.1.255/24");
    expect(result.ipBaseOctet).toBe(50);
  });
});

describe("computeBaseOctet", () => {
  it("computes base from hostname number and IP octet", () => {
    expect(computeBaseOctet("rt-fwd-01", "192.168.1.51/24")).toBe(50);
  });

  it("works for higher hostname numbers", () => {
    expect(computeBaseOctet("rt-fwd-03", "192.168.1.53/24")).toBe(50);
  });

  it("returns 0 for hostname without trailing number", () => {
    expect(computeBaseOctet("my-sbc", "192.168.1.50/24")).toBe(0);
  });

  it("returns 0 for malformed CIDR", () => {
    expect(computeBaseOctet("rt-fwd-01", "badformat")).toBe(0);
  });

  it("clamps to 0 when hostname number exceeds IP octet", () => {
    expect(computeBaseOctet("rt-fwd-05", "192.168.1.2/24")).toBe(0);
  });
});
