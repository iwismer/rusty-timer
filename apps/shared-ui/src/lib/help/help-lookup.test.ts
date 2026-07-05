import { describe, expect, it } from 'vitest';
import { getSection, getField, searchHelp } from './index';
import { FORWARDER_HELP } from './forwarder-help';
import { RECEIVER_HELP } from './receiver-help';
import { RECEIVER_ADMIN_HELP } from './receiver-admin-help';
import { SERVER_HELP } from './server-help';
import type { HelpContextName, HelpContext } from './help-types';

const ALL_CONTEXTS: Record<HelpContextName, HelpContext> = {
  forwarder: FORWARDER_HELP,
  receiver: RECEIVER_HELP,
  'receiver-admin': RECEIVER_ADMIN_HELP,
  server: SERVER_HELP,
};

const htmlTagPattern = /<\/?[a-z][\s>]/i;

function expectPlainText(value: string | undefined, label: string) {
  if (value === undefined) return;
  expect(value, `${label} should be plain text, not HTML`).not.toMatch(htmlTagPattern);
}

describe('getSection', () => {
  it('returns the p2p section for forwarder context', () => {
    const section = getSection('forwarder', 'p2p');
    expect(section).toBeDefined();
    expect(section!.title).toBe('P2P / Server');
  });

  it('returns undefined for a nonexistent section', () => {
    expect(getSection('forwarder', 'nonexistent')).toBeUndefined();
  });
});

describe('getField', () => {
  it('returns the server_url field from forwarder p2p section', () => {
    const field = getField('forwarder', 'p2p', 'server_url');
    expect(field).toBeDefined();
    expect(field!.label).toBe('Server URL');
  });

  it('returns undefined for a nonexistent field', () => {
    expect(getField('forwarder', 'p2p', 'nonexistent')).toBeUndefined();
  });

  it('returns undefined for a nonexistent section', () => {
    expect(getField('forwarder', 'nonexistent', 'server_url')).toBeUndefined();
  });
});

describe('searchHelp', () => {
  it('returns empty array for empty query', () => {
    expect(searchHelp('')).toEqual([]);
  });

  it('returns empty array for whitespace-only query', () => {
    expect(searchHelp('   ')).toEqual([]);
  });

  it('returns empty array when nothing matches', () => {
    expect(searchHelp('zzz-no-match-xyz')).toEqual([]);
  });

  it('finds forwarder p2p section when searching for server content', () => {
    const results = searchHelp('Server URL');
    expect(results.length).toBeGreaterThan(0);
    const match = results.find((r) => r.context === 'forwarder' && r.sectionKey === 'p2p');
    expect(match).toBeDefined();
    expect(match!.matchedFields.some((f) => f.fieldKey === 'server_url')).toBe(true);
  });

  it('matches section title', () => {
    const results = searchHelp('P2P / Server');
    const match = results.find((r) => r.context === 'forwarder' && r.sectionKey === 'p2p');
    expect(match).toBeDefined();
  });

  it('matches case-insensitively', () => {
    const results = searchHelp('SERVER URL');
    expect(results.length).toBeGreaterThan(0);
  });

  it('matches tips', () => {
    const results = searchHelp('descriptive name');
    expect(results.length).toBeGreaterThan(0);
    const match = results.find((r) => r.context === 'forwarder' && r.sectionKey === 'general');
    expect(match).toBeDefined();
    expect(match!.matchedTips.length).toBeGreaterThan(0);
  });

  it('returns all fields when only section title matches', () => {
    const results = searchHelp('P2P / Server');
    const match = results.find((r) => r.context === 'forwarder' && r.sectionKey === 'p2p');
    expect(match).toBeDefined();
    const sectionFieldCount = Object.keys(FORWARDER_HELP.p2p.fields).length;
    expect(match!.matchedFields).toHaveLength(sectionFieldCount);
    expect(match!.matchedFields.some((f) => f.fieldKey === 'server_url')).toBe(true);
  });

  it('matches section overview text', () => {
    const results = searchHelp('IPICO');
    expect(results.length).toBeGreaterThan(0);
    const match = results.find((r) => r.context === 'forwarder' && r.sectionKey === 'readers');
    expect(match).toBeDefined();
  });

  it('handles sections with empty fields (tips-only sections)', () => {
    const results = searchHelp('purge');
    const match = results.find(
      (r) => r.context === 'receiver-admin' && r.sectionKey === 'purge_subscriptions'
    );
    expect(match).toBeDefined();
    expect(match!.matchedTips.length).toBeGreaterThan(0);
  });
});

describe('template wiring validation', () => {
  // All fieldKey+sectionKey+context triples used in HelpTip components across Svelte templates.
  // Update this list when adding new HelpTip usages.
  const expectedFieldLookups: Array<{ context: HelpContextName; section: string; field: string }> =
    [
      // ForwarderConfig.svelte
      { context: 'forwarder', section: 'general', field: 'display_name' },
      { context: 'forwarder', section: 'p2p', field: 'enabled' },
      { context: 'forwarder', section: 'p2p', field: 'server_url' },
      { context: 'forwarder', section: 'p2p', field: 'server_token_file' },
      { context: 'forwarder', section: 'readers', field: 'reader_ip' },
      { context: 'forwarder', section: 'readers', field: 'reader_port' },
      { context: 'forwarder', section: 'readers', field: 'enabled' },
      { context: 'forwarder', section: 'readers', field: 'default_local_port' },
      { context: 'forwarder', section: 'readers', field: 'local_port_override' },
      { context: 'forwarder', section: 'controls', field: 'allow_power_actions' },
      { context: 'forwarder', section: 'auth', field: 'token_file' },
      { context: 'forwarder', section: 'journal', field: 'sqlite_path' },
      { context: 'forwarder', section: 'journal', field: 'prune_watermark_pct' },
      { context: 'forwarder', section: 'status_http', field: 'bind' },
      { context: 'forwarder', section: 'ups', field: 'enabled' },
      { context: 'forwarder', section: 'ups', field: 'daemon_addr' },
      { context: 'forwarder', section: 'ups', field: 'poll_interval_secs' },
      { context: 'forwarder', section: 'ups', field: 'upstream_heartbeat_secs' },
      { context: 'forwarder', section: 'update', field: 'update_mode' },
      // forwarder-ui +page.svelte & legacy dashboard +page.svelte
      { context: 'forwarder', section: 'read_mode', field: 'read_mode' },
      { context: 'forwarder', section: 'read_mode', field: 'timeout' },
      // receiver-ui +page.svelte
      { context: 'receiver', section: 'config', field: 'receiver_id' },
      { context: 'receiver', section: 'config', field: 'server_url' },
      { context: 'receiver', section: 'config', field: 'token' },
      { context: 'receiver', section: 'receiver_mode', field: 'mode' },
      // receiver-ui admin/+page.svelte
      { context: 'receiver-admin', section: 'port_overrides', field: 'port_override' },
      // reader live controls
      { context: 'forwarder', section: 'reader_live', field: 'current_epoch' },
      { context: 'forwarder', section: 'reader_live', field: 'current_epoch_created' },
      { context: 'forwarder', section: 'reader_live', field: 'clock_drift' },
      { context: 'forwarder', section: 'reader_live', field: 'tto_bytes' },
      { context: 'forwarder', section: 'reader_live', field: 'sync_clock' },
      { context: 'forwarder', section: 'reader_live', field: 'refresh_reader' },
      { context: 'forwarder', section: 'reader_live', field: 'recording' },
      { context: 'forwarder', section: 'reader_live', field: 'download_reads' },
      { context: 'forwarder', section: 'reader_live', field: 'clear_records' },
      // server-ui admin
      { context: 'server', section: 'receiver_tokens', field: 'display_name' },
      { context: 'server', section: 'receiver_tokens', field: 'manual_token' },
      { context: 'server', section: 'receiver_tokens', field: 'generate_token' },
      { context: 'server', section: 'receiver_tokens', field: 'add_manual_token' },
      { context: 'server', section: 'receiver_tokens', field: 'one_time_token' },
      { context: 'server', section: 'receiver_tokens', field: 'revoke_token' },
      { context: 'server', section: 'device_approval', field: 'pending_device' },
      { context: 'server', section: 'device_approval', field: 'approve_device' },
      { context: 'server', section: 'device_approval', field: 'approved_device' },
      // server-ui SBC setup
      { context: 'server', section: 'sbc_token_management', field: 'display_name' },
      { context: 'server', section: 'sbc_token_management', field: 'manual_token' },
      { context: 'server', section: 'sbc_token_management', field: 'generate_token' },
      { context: 'server', section: 'sbc_token_management', field: 'add_manual_token' },
      { context: 'server', section: 'sbc_token_management', field: 'one_time_token' },
      { context: 'server', section: 'sbc_token_management', field: 'use_in_setup_form' },
      { context: 'server', section: 'sbc_token_management', field: 'revoke_token' },
      { context: 'server', section: 'sbc_device_identity', field: 'hostname' },
      { context: 'server', section: 'sbc_device_identity', field: 'admin_username' },
      { context: 'server', section: 'sbc_device_identity', field: 'ssh_public_key' },
      { context: 'server', section: 'sbc_network', field: 'static_ipv4_cidr' },
      { context: 'server', section: 'sbc_network', field: 'gateway' },
      { context: 'server', section: 'sbc_network', field: 'dns_servers' },
      { context: 'server', section: 'sbc_network', field: 'wifi_enabled' },
      { context: 'server', section: 'sbc_network', field: 'wifi_ssid' },
      { context: 'server', section: 'sbc_network', field: 'wifi_country' },
      { context: 'server', section: 'sbc_network', field: 'wifi_password' },
      { context: 'server', section: 'sbc_forwarder_setup', field: 'server_url' },
      { context: 'server', section: 'sbc_forwarder_setup', field: 'auth_token' },
      { context: 'server', section: 'sbc_forwarder_setup', field: 'display_name' },
      { context: 'server', section: 'sbc_forwarder_setup', field: 'reader_targets' },
      { context: 'server', section: 'sbc_advanced', field: 'status_bind' },
      { context: 'server', section: 'sbc_advanced', field: 'setup_script_url' },
      { context: 'server', section: 'sbc_advanced', field: 'ups_enabled' },
      { context: 'server', section: 'sbc_download_actions', field: 'download_user_data' },
      { context: 'server', section: 'sbc_download_actions', field: 'download_network_config' },
      { context: 'server', section: 'sbc_download_actions', field: 'save_next_device' },
    ];

  it.each(expectedFieldLookups)(
    'resolves $context/$section/$field',
    ({ context, section, field }) => {
      expect(getField(context, section, field)).toBeDefined();
    }
  );

  // All helpSection+helpContext pairs used on Card components.
  const expectedSectionLookups: Array<{ context: HelpContextName; section: string }> = [
    // ForwarderConfig.svelte
    { context: 'forwarder', section: 'general' },
    { context: 'forwarder', section: 'p2p' },
    { context: 'forwarder', section: 'readers' },
    { context: 'forwarder', section: 'controls' },
    { context: 'forwarder', section: 'dangerous_actions' },
    { context: 'forwarder', section: 'auth' },
    { context: 'forwarder', section: 'journal' },
    { context: 'forwarder', section: 'status_http' },
    { context: 'forwarder', section: 'ups' },
    { context: 'forwarder', section: 'update' },
    // forwarder-ui & legacy dashboard +page.svelte (HelpDialog usage)
    { context: 'forwarder', section: 'read_mode' },
    // receiver-ui +page.svelte
    { context: 'receiver', section: 'config' },
    { context: 'receiver', section: 'receiver_mode' },
    { context: 'receiver', section: 'streams' },
    // receiver-ui admin/+page.svelte
    { context: 'receiver-admin', section: 'cursor_reset' },
    { context: 'receiver-admin', section: 'epoch_overrides' },
    { context: 'receiver-admin', section: 'port_overrides' },
    { context: 'receiver-admin', section: 'purge_subscriptions' },
    { context: 'receiver-admin', section: 'reset_profile' },
    { context: 'receiver-admin', section: 'factory_reset' },
    // reader live controls
    { context: 'forwarder', section: 'reader_live' },
    // server-ui
    { context: 'server', section: 'server_status' },
    { context: 'server', section: 'stream_catalogs' },
    { context: 'server', section: 'registered_devices' },
    { context: 'server', section: 'receiver_tokens' },
    { context: 'server', section: 'device_approval' },
    { context: 'server', section: 'sbc_token_management' },
    { context: 'server', section: 'sbc_device_identity' },
    { context: 'server', section: 'sbc_network' },
    { context: 'server', section: 'sbc_forwarder_setup' },
    { context: 'server', section: 'sbc_advanced' },
    { context: 'server', section: 'sbc_download_actions' },
  ];

  it.each(expectedSectionLookups)('resolves section $context/$section', ({ context, section }) => {
    expect(getSection(context, section)).toBeDefined();
  });
});

describe('help content validation', () => {
  it('keeps plain-text help fields free of HTML markup', () => {
    for (const [contextName, context] of Object.entries(ALL_CONTEXTS)) {
      for (const [sectionKey, section] of Object.entries(context)) {
        expectPlainText(section.title, `${contextName}.${sectionKey}.title`);
        expectPlainText(section.overview, `${contextName}.${sectionKey}.overview`);
        for (const [fieldKey, field] of Object.entries(section.fields)) {
          expectPlainText(field.label, `${contextName}.${sectionKey}.${fieldKey}.label`);
          expectPlainText(field.summary, `${contextName}.${sectionKey}.${fieldKey}.summary`);
          expectPlainText(field.default, `${contextName}.${sectionKey}.${fieldKey}.default`);
          expectPlainText(field.range, `${contextName}.${sectionKey}.${fieldKey}.range`);
          expectPlainText(
            field.recommended,
            `${contextName}.${sectionKey}.${fieldKey}.recommended`
          );
        }
      }
    }
  });

  it('all seeAlso references resolve to existing sections', () => {
    const errors: string[] = [];
    for (const [contextName, context] of Object.entries(ALL_CONTEXTS)) {
      for (const [sectionKey, section] of Object.entries(context)) {
        for (const link of section.seeAlso ?? []) {
          if (!context[link.sectionKey]) {
            errors.push(
              `${contextName}/${sectionKey} -> seeAlso "${link.sectionKey}" does not exist`
            );
          }
        }
      }
    }
    expect(errors).toEqual([]);
  });
});
