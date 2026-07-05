import { describe, expect, it } from "vitest";
import adminTab from "./AdminTab.svelte?raw";
import announcerTab from "./AnnouncerTab.svelte?raw";
import configTab from "./ConfigTab.svelte?raw";
import connectionsTab from "./ConnectionsTab.svelte?raw";
import statusBar from "./StatusBar.svelte?raw";
import streamsTab from "./StreamsTab.svelte?raw";

const components: Record<string, string> = {
  AdminTab: adminTab,
  AnnouncerTab: announcerTab,
  ConfigTab: configTab,
  ConnectionsTab: connectionsTab,
  StatusBar: statusBar,
  StreamsTab: streamsTab,
};

function readComponent(name: string): string {
  return components[name];
}

function expectHelpTips(source: string, fields: string[]) {
  for (const field of fields) {
    expect(source, `missing HelpTip for ${field}`).toContain(
      `fieldKey="${field}"`,
    );
  }
}

describe("receiver UI help coverage", () => {
  it("wires config and Race Director fields to help", () => {
    const source = readComponent("ConfigTab");
    expectHelpTips(source, [
      "receiver_id",
      "server_url",
      "token",
      "rd_import_enabled",
      "rd_import_dir",
      "rd_import_interval",
      "dbf_enabled",
      "dbf_flush_interval",
      "clear_dbf",
    ]);
    expect(source).toContain("onOpenModal={openHelp}");
  });

  it("wires connections fields and actions to help", () => {
    const source = readComponent("ConnectionsTab");
    expectHelpTips(source, [
      "server_status",
      "open_admin_panel",
      "forwarder_state",
      "forwarder_actions",
      "forwarder_configure",
      "forwarder_battery",
      "reader_controls",
    ]);
    expect(source).toContain("onOpenModal={openHelp}");
  });

  it("wires streams fields and actions to help", () => {
    const source = readComponent("StreamsTab");
    expectHelpTips(source, [
      "stream_identity",
      "status_indicator",
      "last_read",
      "reads",
      "local_port",
      "stream_epoch",
      "stream_metrics",
      "event_type",
      "earliest_epoch",
      "announce",
      "replay",
      "subscribed",
      "subscribe_all",
    ]);
    expect(source).toContain("onOpenModal={openHelp}");
  });

  it("wires announcer fields and stats to help", () => {
    const source = readComponent("AnnouncerTab");
    expectHelpTips(source, [
      "announcer_enabled",
      "max_list_size",
      "open_announcer_page",
      "participants_file",
      "chips_file",
      "data_stats",
      "rd_auto_import",
    ]);
    expect(source).toContain("onOpenModal={openHelp}");
  });

  it("wires in-app admin sections and actions to help", () => {
    const source = readComponent("AdminTab");
    expectHelpTips(source, [
      "stream_cursor",
      "reset_cursor",
      "reset_all_cursors",
      "epoch_override",
      "reset_epoch_override",
      "reset_all_epoch_overrides",
      "port_override",
      "purge_all_subscriptions",
      "reset_profile_action",
      "clear_data_action",
      "factory_reset_action",
    ]);
    expect(source).toContain("onOpenModal={openHelp}");
  });

  it("wires status bar summary values to help", () => {
    const source = readComponent("StatusBar");
    expectHelpTips(source, [
      "overall_health",
      "total_reads",
      "identity_version",
    ]);
    expect(source).toContain("onOpenModal={openHelp}");
  });
});
