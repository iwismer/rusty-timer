import { describe, expect, it } from "vitest";

import layoutSource from "../routes/+layout.svelte?raw";
import statusSource from "../routes/+page.svelte?raw";
import adminSource from "../routes/admin/+page.svelte?raw";
import sbcSetupSource from "../routes/sbc-setup/+page.svelte?raw";

function expectHelpSection(source: string, section: string) {
  expect(source, `missing helpSection=${section}`).toContain(
    `helpSection="${section}"`,
  );
  expect(source, `missing server help context for ${section}`).toContain(
    'helpContext="server"',
  );
}

function expectHelpTip(source: string, section: string, field: string) {
  const pattern = new RegExp(
    `<HelpTip\\s+fieldKey="${field}"\\s+sectionKey="${section}"\\s+context="server"`,
  );
  expect(source, `missing HelpTip for ${section}/${field}`).toMatch(pattern);
}

function expectButtonHelpTipGroup(
  source: string,
  buttonText: string,
  section: string,
  field: string,
) {
  const escapedButtonText = buttonText.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const pattern = new RegExp(
    `<div\\s+class="[^"]*flex[^"]*"[^>]*>(?:(?!</div>)[\\s\\S])*${escapedButtonText}(?:(?!</div>)[\\s\\S])*<HelpTip\\s+fieldKey="${field}"\\s+sectionKey="${section}"\\s+context="server"(?:(?!</div>)[\\s\\S])*</div>`,
  );
  expect(
    source,
    `expected ${section}/${field} HelpTip to be grouped with the ${buttonText} button`,
  ).toMatch(pattern);
}

describe("server UI help wiring", () => {
  it("enables global server help search in the shared NavBar", () => {
    expect(layoutSource).toContain('helpContext="server"');
  });

  it("wires status dashboard cards to server help", () => {
    for (const section of [
      "server_status",
      "stream_catalogs",
      "registered_devices",
    ]) {
      expectHelpSection(statusSource, section);
    }
  });

  it("wires admin enrollment and approval controls to server help", () => {
    expectHelpSection(adminSource, "receiver_tokens");
    expectHelpSection(adminSource, "device_approval");
    for (const [section, fields] of Object.entries({
      receiver_tokens: [
        "display_name",
        "manual_token",
        "generate_token",
        "add_manual_token",
        "one_time_token",
        "revoke_token",
      ],
      device_approval: ["pending_device", "approve_device", "approved_device"],
    })) {
      for (const field of fields) {
        expectHelpTip(adminSource, section, field);
      }
    }

    expectButtonHelpTipGroup(
      adminSource,
      "Revoke",
      "receiver_tokens",
      "revoke_token",
    );
    expectButtonHelpTipGroup(
      adminSource,
      "Approve",
      "device_approval",
      "approve_device",
    );
  });

  it("wires SBC setup sections and labels to server help", () => {
    for (const section of [
      "sbc_token_management",
      "sbc_device_identity",
      "sbc_network",
      "sbc_forwarder_setup",
      "sbc_advanced",
      "sbc_download_actions",
    ]) {
      expectHelpSection(sbcSetupSource, section);
    }

    for (const [section, fields] of Object.entries({
      sbc_token_management: [
        "display_name",
        "manual_token",
        "generate_token",
        "add_manual_token",
        "one_time_token",
        "use_in_setup_form",
        "revoke_token",
      ],
      sbc_device_identity: ["hostname", "admin_username", "ssh_public_key"],
      sbc_network: [
        "static_ipv4_cidr",
        "gateway",
        "dns_servers",
        "wifi_enabled",
        "wifi_ssid",
        "wifi_country",
        "wifi_password",
      ],
      sbc_forwarder_setup: [
        "server_url",
        "auth_token",
        "display_name",
        "reader_targets",
      ],
      sbc_advanced: ["status_bind", "setup_script_url", "ups_enabled"],
      sbc_download_actions: [
        "download_user_data",
        "download_network_config",
        "save_next_device",
      ],
    })) {
      for (const field of fields) {
        expectHelpTip(sbcSetupSource, section, field);
      }
    }
  });
});
