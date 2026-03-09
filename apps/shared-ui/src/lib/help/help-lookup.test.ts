import { describe, expect, it } from "vitest";
import { getSection, getField, searchHelp } from "./index";
import { FORWARDER_HELP } from "./forwarder-help";
import { RECEIVER_HELP } from "./receiver-help";
import { RECEIVER_ADMIN_HELP } from "./receiver-admin-help";
import { SERVER_HELP } from "./server-help";
import type { HelpContextName, HelpContext } from "./help-types";

describe("getSection", () => {
  it("returns the server section for forwarder context", () => {
    const section = getSection("forwarder", "server");
    expect(section).toBeDefined();
    expect(section!.title).toBe("Server Connection");
  });

  it("returns undefined for a nonexistent section", () => {
    expect(getSection("forwarder", "nonexistent")).toBeUndefined();
  });
});

describe("getField", () => {
  it("returns the base_url field from forwarder server section", () => {
    const field = getField("forwarder", "server", "base_url");
    expect(field).toBeDefined();
    expect(field!.label).toBe("Base URL");
  });

  it("returns undefined for a nonexistent field", () => {
    expect(getField("forwarder", "server", "nonexistent")).toBeUndefined();
  });

  it("returns undefined for a nonexistent section", () => {
    expect(getField("forwarder", "nonexistent", "base_url")).toBeUndefined();
  });
});

describe("searchHelp", () => {
  it("returns empty array for empty query", () => {
    expect(searchHelp("")).toEqual([]);
  });

  it("returns empty array for whitespace-only query", () => {
    expect(searchHelp("   ")).toEqual([]);
  });

  it("returns empty array when nothing matches", () => {
    expect(searchHelp("zzz-no-match-xyz")).toEqual([]);
  });

  it("finds forwarder server section when searching for base_url content", () => {
    const results = searchHelp("Base URL");
    expect(results.length).toBeGreaterThan(0);
    const match = results.find(
      (r) => r.context === "forwarder" && r.sectionKey === "server",
    );
    expect(match).toBeDefined();
    expect(match!.matchedFields.some((f) => f.fieldKey === "base_url")).toBe(true);
  });

  it("matches section title", () => {
    const results = searchHelp("Server Connection");
    const match = results.find(
      (r) => r.context === "forwarder" && r.sectionKey === "server",
    );
    expect(match).toBeDefined();
  });

  it("matches case-insensitively", () => {
    const results = searchHelp("BASE URL");
    expect(results.length).toBeGreaterThan(0);
  });

  it("matches tips", () => {
    const results = searchHelp("descriptive name");
    expect(results.length).toBeGreaterThan(0);
    const match = results.find(
      (r) => r.context === "forwarder" && r.sectionKey === "general",
    );
    expect(match).toBeDefined();
    expect(match!.matchedTips.length).toBeGreaterThan(0);
  });

  it("returns all fields when only section title matches", () => {
    const results = searchHelp("Server Connection");
    const match = results.find(
      (r) => r.context === "forwarder" && r.sectionKey === "server",
    );
    expect(match).toBeDefined();
    const sectionFieldCount = Object.keys(FORWARDER_HELP.server.fields).length;
    expect(match!.matchedFields).toHaveLength(sectionFieldCount);
    expect(match!.matchedFields.some((f) => f.fieldKey === "base_url")).toBe(true);
  });

  it("matches section overview text", () => {
    const results = searchHelp("IPICO");
    expect(results.length).toBeGreaterThan(0);
    const match = results.find(
      (r) => r.context === "forwarder" && r.sectionKey === "readers",
    );
    expect(match).toBeDefined();
  });

  it("handles sections with empty fields (tips-only sections)", () => {
    const results = searchHelp("purge");
    const match = results.find(
      (r) => r.context === "receiver-admin" && r.sectionKey === "purge_subscriptions",
    );
    expect(match).toBeDefined();
    expect(match!.matchedTips.length).toBeGreaterThan(0);
  });
});

describe("template wiring validation", () => {
  // All fieldKey+sectionKey+context triples used in HelpTip components across Svelte templates.
  // Update this list when adding new HelpTip usages.
  const expectedFieldLookups: Array<{ context: HelpContextName; section: string; field: string }> = [
    // ForwarderConfig.svelte
    { context: "forwarder", section: "general", field: "display_name" },
    { context: "forwarder", section: "server", field: "base_url" },
    { context: "forwarder", section: "readers", field: "reader_ip" },
    { context: "forwarder", section: "readers", field: "reader_port" },
    { context: "forwarder", section: "readers", field: "enabled" },
    { context: "forwarder", section: "readers", field: "default_local_port" },
    { context: "forwarder", section: "readers", field: "local_port_override" },
    { context: "forwarder", section: "controls", field: "allow_power_actions" },
    { context: "forwarder", section: "ws_path", field: "forwarders_ws_path" },
    { context: "forwarder", section: "auth", field: "token_file" },
    { context: "forwarder", section: "journal", field: "sqlite_path" },
    { context: "forwarder", section: "journal", field: "prune_watermark_pct" },
    { context: "forwarder", section: "uplink", field: "batch_mode" },
    { context: "forwarder", section: "uplink", field: "batch_flush_ms" },
    { context: "forwarder", section: "uplink", field: "batch_max_events" },
    { context: "forwarder", section: "status_http", field: "bind" },
    { context: "forwarder", section: "update", field: "update_mode" },
    // forwarder-ui +page.svelte & server-ui +page.svelte
    { context: "forwarder", section: "read_mode", field: "read_mode" },
    { context: "forwarder", section: "read_mode", field: "timeout" },
    // receiver-ui +page.svelte
    { context: "receiver", section: "config", field: "receiver_id" },
    { context: "receiver", section: "config", field: "server_url" },
    { context: "receiver", section: "config", field: "token" },
    { context: "receiver", section: "config", field: "update_mode" },
    { context: "receiver", section: "receiver_mode", field: "mode" },
    // receiver-ui admin/+page.svelte
    { context: "receiver-admin", section: "port_overrides", field: "port_override" },
    // server-ui +page.svelte (stream filters)
    { context: "server", section: "stream_filters", field: "race_filter" },
    { context: "server", section: "stream_filters", field: "hide_offline" },
    { context: "server", section: "stream_filters", field: "forwarder_race" },
    // server-ui +page.svelte (reader live — context is forwarder)
    { context: "forwarder", section: "reader_live", field: "clock_drift" },
    { context: "forwarder", section: "reader_live", field: "tto_bytes" },
    { context: "forwarder", section: "reader_live", field: "sync_clock" },
    { context: "forwarder", section: "reader_live", field: "refresh_reader" },
    { context: "forwarder", section: "reader_live", field: "recording" },
    { context: "forwarder", section: "reader_live", field: "download_reads" },
    { context: "forwarder", section: "reader_live", field: "clear_records" },
    // server-ui announcer-config/+page.svelte
    { context: "server", section: "announcer", field: "enabled" },
    { context: "server", section: "announcer", field: "streams" },
    { context: "server", section: "announcer", field: "max_list_size" },
    { context: "server", section: "announcer", field: "reset" },
    // server-ui sbc-setup/+page.svelte
    { context: "server", section: "sbc_identity", field: "hostname" },
    { context: "server", section: "sbc_identity", field: "admin_username" },
    { context: "server", section: "sbc_identity", field: "ssh_public_key" },
    { context: "server", section: "sbc_network", field: "static_ipv4" },
    { context: "server", section: "sbc_network", field: "gateway" },
    { context: "server", section: "sbc_network", field: "dns_servers" },
    { context: "server", section: "sbc_network", field: "wifi_enabled" },
    { context: "server", section: "sbc_network", field: "wifi_ssid" },
    { context: "server", section: "sbc_network", field: "wifi_password" },
    { context: "server", section: "sbc_network", field: "wifi_country" },
    { context: "server", section: "sbc_forwarder", field: "server_base_url" },
    { context: "server", section: "sbc_forwarder", field: "auth_token" },
    { context: "server", section: "sbc_forwarder", field: "reader_targets" },
    { context: "server", section: "sbc_forwarder", field: "status_bind" },
    { context: "server", section: "sbc_forwarder", field: "display_name" },
    { context: "server", section: "sbc_advanced", field: "setup_script_url" },
  ];

  it.each(expectedFieldLookups)(
    "resolves $context/$section/$field",
    ({ context, section, field }) => {
      expect(getField(context, section, field)).toBeDefined();
    },
  );

  // All helpSection+helpContext pairs used on Card components.
  const expectedSectionLookups: Array<{ context: HelpContextName; section: string }> = [
    // ForwarderConfig.svelte
    { context: "forwarder", section: "general" },
    { context: "forwarder", section: "server" },
    { context: "forwarder", section: "readers" },
    { context: "forwarder", section: "controls" },
    { context: "forwarder", section: "dangerous_actions" },
    { context: "forwarder", section: "ws_path" },
    { context: "forwarder", section: "auth" },
    { context: "forwarder", section: "journal" },
    { context: "forwarder", section: "uplink" },
    { context: "forwarder", section: "status_http" },
    { context: "forwarder", section: "update" },
    // forwarder-ui & server-ui +page.svelte (HelpDialog usage)
    { context: "forwarder", section: "read_mode" },
    // receiver-ui +page.svelte
    { context: "receiver", section: "config" },
    { context: "receiver", section: "receiver_mode" },
    { context: "receiver", section: "streams" },
    // receiver-ui admin/+page.svelte
    { context: "receiver-admin", section: "cursor_reset" },
    { context: "receiver-admin", section: "epoch_overrides" },
    { context: "receiver-admin", section: "port_overrides" },
    { context: "receiver-admin", section: "purge_subscriptions" },
    { context: "receiver-admin", section: "reset_profile" },
    { context: "receiver-admin", section: "factory_reset" },
    // server-ui +page.svelte
    { context: "server", section: "stream_filters" },
    { context: "forwarder", section: "reader_live" },
    // server-ui announcer-config/+page.svelte
    { context: "server", section: "announcer" },
    // server-ui sbc-setup/+page.svelte
    { context: "server", section: "sbc_identity" },
    { context: "server", section: "sbc_network" },
    { context: "server", section: "sbc_forwarder" },
    { context: "server", section: "sbc_advanced" },
    // server-ui admin/+page.svelte
    { context: "server", section: "admin_streams" },
    { context: "server", section: "admin_events" },
    { context: "server", section: "admin_tokens" },
    { context: "server", section: "admin_cursors" },
    { context: "server", section: "admin_races" },
    // server-ui races/+page.svelte
    { context: "server", section: "races" },
    // server-ui races/[raceId]/+page.svelte
    { context: "server", section: "race_detail" },
  ];

  it.each(expectedSectionLookups)(
    "resolves section $context/$section",
    ({ context, section }) => {
      expect(getSection(context, section)).toBeDefined();
    },
  );
});

describe("seeAlso cross-reference validation", () => {
  const contexts: Record<HelpContextName, HelpContext> = {
    forwarder: FORWARDER_HELP,
    receiver: RECEIVER_HELP,
    "receiver-admin": RECEIVER_ADMIN_HELP,
    server: SERVER_HELP,
  };

  it("all seeAlso references resolve to existing sections", () => {
    const errors: string[] = [];
    for (const [contextName, context] of Object.entries(contexts)) {
      for (const [sectionKey, section] of Object.entries(context)) {
        for (const link of section.seeAlso ?? []) {
          if (!context[link.sectionKey]) {
            errors.push(
              `${contextName}/${sectionKey} -> seeAlso "${link.sectionKey}" does not exist`,
            );
          }
        }
      }
    }
    expect(errors).toEqual([]);
  });
});
