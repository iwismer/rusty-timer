import { describe, expect, it } from "vitest";
import { generateNetworkConfig, generateUserData } from "./generate";
import type { SbcSetupFormData } from "./types";

const baseForm: SbcSetupFormData = {
  hostname: "rt-fwd-01",
  adminUsername: "rt-admin",
  sshPublicKey: "ssh-ed25519 AAAA test",
  staticIpv4Cidr: "192.168.1.51/24",
  gateway: "192.168.1.1",
  dnsServers: "8.8.8.8,8.8.4.4",
  wifiEnabled: false,
  wifiSsid: "",
  wifiPassword: "",
  wifiCountry: "CA",
  serverUrl: "https://timer.example.com",
  authToken: "token-secret",
  readerTargets: "192.168.1.10:10000\n192.168.1.11:10000",
  statusBind: "0.0.0.0:80",
  displayName: "Start Line",
  setupScriptUrl:
    "https://raw.githubusercontent.com/iwismer/rusty-timer/main/deploy/sbc/rt-setup.sh",
  upsEnabled: false,
};

describe("sbc cloud-init generation", () => {
  it("user_data_uses_current_server_url_env_name", () => {
    const text = generateUserData(baseForm);

    expect(text).toContain("RT_SETUP_SERVER_URL=");
    expect(text).not.toContain("RT_SETUP_SERVER_BASE_URL");
  });

  it("user_data_uses_main_setup_script_url", () => {
    const text = generateUserData(baseForm);

    expect(text).toContain("/rusty-timer/main/deploy/sbc/rt-setup.sh");
    expect(text).not.toContain("/rusty-timer/master/");
  });

  it("user_data_includes_spi_i2c_bootcmd", () => {
    const text = generateUserData(baseForm);

    expect(text).toContain("bootcmd:");
    expect(text).toContain("dtparam=spi=on");
    expect(text).toContain("dtparam=i2c_arm=on");
  });

  it("user_data_includes_ups_env_and_package_when_enabled", () => {
    const text = generateUserData({ ...baseForm, upsEnabled: true });

    expect(text).toContain("  - i2c-tools");
    expect(text).toContain("RT_SETUP_UPS_ENABLED=1");
  });

  it("network_config_includes_metric_600", () => {
    const text = generateNetworkConfig(baseForm);

    expect(text).toContain("          metric: 600");
  });

  it("network_config_omits_wifi_when_disabled", () => {
    const text = generateNetworkConfig(baseForm);

    expect(text).not.toContain("wifis:");
  });

  it("network_config_renders_wifi_with_password", () => {
    const text = generateNetworkConfig({
      ...baseForm,
      wifiEnabled: true,
      wifiSsid: "Race WiFi",
      wifiPassword: "secret",
      wifiCountry: "ca",
    });

    expect(text).toContain("  wifis:");
    expect(text).toContain("      regulatory-domain: 'CA'");
    expect(text).toContain("        'Race WiFi':");
    expect(text).toContain("          password: 'secret'");
  });

  it("network_config_renders_open_wifi", () => {
    const text = generateNetworkConfig({
      ...baseForm,
      wifiEnabled: true,
      wifiSsid: "Open Race",
      wifiPassword: "",
    });

    expect(text).toContain("        'Open Race': {}\n");
  });

  it("quotes_single_quotes_safely", () => {
    const text = generateUserData({
      ...baseForm,
      authToken: "abc'def",
      displayName: "Start's Line",
    });

    expect(text).toContain("RT_SETUP_AUTH_TOKEN='abc'\"'\"'def'");
    expect(text).toContain("RT_SETUP_DISPLAY_NAME='Start'\"'\"'s Line'");
  });
});
