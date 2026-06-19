import { describe, expect, it } from "vitest";
import { getSection, getField, searchHelp } from "./index";
import { FORWARDER_HELP } from "./forwarder-help";
import { RECEIVER_HELP } from "./receiver-help";
import { RECEIVER_ADMIN_HELP } from "./receiver-admin-help";
import type { HelpContextName, HelpContext } from "./help-types";

describe("getSection", () => {
  it("returns the p2p section for forwarder context", () => {
    const section = getSection("forwarder", "p2p");
    expect(section).toBeDefined();
    expect(section!.title).toBe("P2P / Server");
  });

  it("returns undefined for a nonexistent section", () => {
    expect(getSection("forwarder", "nonexistent")).toBeUndefined();
  });
});

describe("getField", () => {
  it("returns the server_url field from forwarder p2p section", () => {
    const field = getField("forwarder", "p2p", "server_url");
    expect(field).toBeDefined();
    expect(field!.label).toBe("Server URL");
  });

  it("returns undefined for a nonexistent field", () => {
    expect(getField("forwarder", "p2p", "nonexistent")).toBeUndefined();
  });

  it("returns undefined for a nonexistent section", () => {
    expect(getField("forwarder", "nonexistent", "server_url")).toBeUndefined();
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

  it("finds forwarder p2p section when searching for server content", () => {
    const results = searchHelp("Server URL");
    expect(results.length).toBeGreaterThan(0);
    const match = results.find(
      (r) => r.context === "forwarder" && r.sectionKey === "p2p",
    );
    expect(match).toBeDefined();
    expect(match!.matchedFields.some((f) => f.fieldKey === "server_url")).toBe(true);
  });

  it("matches section title", () => {
    const results = searchHelp("P2P / Server");
    const match = results.find(
      (r) => r.context === "forwarder" && r.sectionKey === "p2p",
    );
    expect(match).toBeDefined();
  });

  it("matches case-insensitively", () => {
    const results = searchHelp("SERVER URL");
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
    const results = searchHelp("P2P / Server");
    const match = results.find(
      (r) => r.context === "forwarder" && r.sectionKey === "p2p",
    );
    expect(match).toBeDefined();
    const sectionFieldCount = Object.keys(FORWARDER_HELP.p2p.fields).length;
    expect(match!.matchedFields).toHaveLength(sectionFieldCount);
    expect(match!.matchedFields.some((f) => f.fieldKey === "server_url")).toBe(true);
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
    { context: "forwarder", section: "p2p", field: "enabled" },
    { context: "forwarder", section: "p2p", field: "server_url" },
    { context: "forwarder", section: "p2p", field: "server_token_file" },
    { context: "forwarder", section: "readers", field: "reader_ip" },
    { context: "forwarder", section: "readers", field: "reader_port" },
    { context: "forwarder", section: "readers", field: "enabled" },
    { context: "forwarder", section: "readers", field: "default_local_port" },
    { context: "forwarder", section: "readers", field: "local_port_override" },
    { context: "forwarder", section: "controls", field: "allow_power_actions" },
    { context: "forwarder", section: "auth", field: "token_file" },
    { context: "forwarder", section: "journal", field: "sqlite_path" },
    { context: "forwarder", section: "journal", field: "prune_watermark_pct" },
    { context: "forwarder", section: "status_http", field: "bind" },
    { context: "forwarder", section: "update", field: "update_mode" },
    // forwarder-ui +page.svelte & legacy dashboard +page.svelte
    { context: "forwarder", section: "read_mode", field: "read_mode" },
    { context: "forwarder", section: "read_mode", field: "timeout" },
    // receiver-ui +page.svelte
    { context: "receiver", section: "config", field: "receiver_id" },
    { context: "receiver", section: "config", field: "server_url" },
    { context: "receiver", section: "config", field: "token" },
    { context: "receiver", section: "receiver_mode", field: "mode" },
    // receiver-ui admin/+page.svelte
    { context: "receiver-admin", section: "port_overrides", field: "port_override" },
    // reader live controls
    { context: "forwarder", section: "reader_live", field: "clock_drift" },
    { context: "forwarder", section: "reader_live", field: "tto_bytes" },
    { context: "forwarder", section: "reader_live", field: "sync_clock" },
    { context: "forwarder", section: "reader_live", field: "refresh_reader" },
    { context: "forwarder", section: "reader_live", field: "recording" },
    { context: "forwarder", section: "reader_live", field: "download_reads" },
    { context: "forwarder", section: "reader_live", field: "clear_records" },
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
    { context: "forwarder", section: "p2p" },
    { context: "forwarder", section: "readers" },
    { context: "forwarder", section: "controls" },
    { context: "forwarder", section: "dangerous_actions" },
    { context: "forwarder", section: "auth" },
    { context: "forwarder", section: "journal" },
    { context: "forwarder", section: "status_http" },
    { context: "forwarder", section: "update" },
    // forwarder-ui & legacy dashboard +page.svelte (HelpDialog usage)
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
    // reader live controls
    { context: "forwarder", section: "reader_live" },
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
