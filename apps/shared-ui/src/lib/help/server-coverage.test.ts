import { describe, expect, it } from 'vitest';
import { SERVER_HELP } from './server-help';

function expectFields(section: { fields: Record<string, unknown> }, fields: string[]) {
  for (const field of fields) {
    expect(section.fields, `missing field ${field}`).toHaveProperty(field);
  }
}

const htmlTagPattern = /<\/?[a-z][a-z0-9-]*(?:\s|>|\/>)/i;

function expectPlainText(value: string | undefined, label: string) {
  if (value === undefined) return;
  expect(value, `${label} should be plain text, not HTML`).not.toMatch(htmlTagPattern);
}

describe('server help coverage', () => {
  it('covers status dashboard sections', () => {
    expect(SERVER_HELP).toHaveProperty('server_status');
    expectFields(SERVER_HELP.server_status, [
      'announcer_generation',
      'finisher_count',
      'registered_devices_count',
    ]);

    expect(SERVER_HELP).toHaveProperty('stream_catalogs');
    expectFields(SERVER_HELP.stream_catalogs, [
      'forwarder',
      'stream',
      'epoch',
      'next_seq',
      'approval',
    ]);

    expect(SERVER_HELP).toHaveProperty('registered_devices');
    expectFields(SERVER_HELP.registered_devices, ['endpoint', 'kind', 'approval_state']);
  });

  it('covers admin enrollment and approval sections', () => {
    expect(SERVER_HELP).toHaveProperty('receiver_tokens');
    expectFields(SERVER_HELP.receiver_tokens, [
      'display_name',
      'manual_token',
      'generate_token',
      'add_manual_token',
      'one_time_token',
      'token_status',
      'used_by_endpoint',
      'revoke_token',
    ]);

    expect(SERVER_HELP).toHaveProperty('device_approval');
    expectFields(SERVER_HELP.device_approval, [
      'pending_device',
      'approve_device',
      'approved_device',
      'endpoint_id',
    ]);
  });

  it('covers SBC setup sections', () => {
    expectFields(SERVER_HELP.sbc_token_management, [
      'display_name',
      'manual_token',
      'generate_token',
      'add_manual_token',
      'one_time_token',
      'use_in_setup_form',
      'revoke_token',
    ]);
    expectFields(SERVER_HELP.sbc_device_identity, ['hostname', 'admin_username', 'ssh_public_key']);
    expectFields(SERVER_HELP.sbc_network, [
      'static_ipv4_cidr',
      'gateway',
      'dns_servers',
      'wifi_enabled',
      'wifi_ssid',
      'wifi_country',
      'wifi_password',
    ]);
    expectFields(SERVER_HELP.sbc_forwarder_setup, [
      'server_url',
      'auth_token',
      'display_name',
      'reader_targets',
    ]);
    expectFields(SERVER_HELP.sbc_advanced, ['status_bind', 'setup_script_url', 'ups_enabled']);
    expectFields(SERVER_HELP.sbc_download_actions, [
      'download_user_data',
      'download_network_config',
      'save_next_device',
    ]);
  });

  it('documents one-time token and Save & Next Device safety behavior', () => {
    expect(SERVER_HELP.sbc_token_management.fields.one_time_token.detailHtml).toMatch(
      /shown only once/i
    );
    expect(SERVER_HELP.sbc_download_actions.fields.save_next_device.detailHtml).toMatch(
      /clears? the auth token/i
    );
  });

  it('rejects multi-letter HTML tags in server plain-text fields', () => {
    expect(() => expectPlainText('<strong>not plain</strong>', 'example.summary')).toThrow();
  });

  it('keeps server plain-text help fields free of HTML markup', () => {
    for (const [sectionKey, section] of Object.entries(SERVER_HELP)) {
      expectPlainText(section.title, `${sectionKey}.title`);
      expectPlainText(section.overview, `${sectionKey}.overview`);
      for (const [fieldKey, field] of Object.entries(section.fields)) {
        expectPlainText(field.label, `${sectionKey}.${fieldKey}.label`);
        expectPlainText(field.summary, `${sectionKey}.${fieldKey}.summary`);
        expectPlainText(field.default, `${sectionKey}.${fieldKey}.default`);
        expectPlainText(field.range, `${sectionKey}.${fieldKey}.range`);
        expectPlainText(field.recommended, `${sectionKey}.${fieldKey}.recommended`);
      }
    }
  });
});
