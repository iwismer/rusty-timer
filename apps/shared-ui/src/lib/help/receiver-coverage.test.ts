import { describe, expect, it } from 'vitest';
import { RECEIVER_ADMIN_HELP } from './receiver-admin-help';
import { RECEIVER_HELP } from './receiver-help';

function expectFields(section: { fields: Record<string, unknown> }, fields: string[]) {
  for (const field of fields) {
    expect(section.fields, `missing field ${field}`).toHaveProperty(field);
  }
}

describe('receiver help coverage', () => {
  it('covers receiver operator tabs', () => {
    expect(RECEIVER_HELP).toHaveProperty('connections');
    expectFields(RECEIVER_HELP.connections, [
      'server_status',
      'open_admin_panel',
      'forwarder_state',
      'forwarder_actions',
      'forwarder_configure',
      'forwarder_battery',
      'reader_controls',
    ]);

    expectFields(RECEIVER_HELP.streams, [
      'status_indicator',
      'last_read',
      'reads',
      'stream_metrics',
      'event_type',
      'announce',
      'subscribe_all',
    ]);

    expect(RECEIVER_HELP).toHaveProperty('announcer');
    expectFields(RECEIVER_HELP.announcer, [
      'announcer_enabled',
      'max_list_size',
      'open_announcer_page',
      'participants_file',
      'chips_file',
      'data_stats',
      'rd_auto_import',
    ]);

    expect(RECEIVER_HELP).toHaveProperty('status_bar');
    expectFields(RECEIVER_HELP.status_bar, ['overall_health', 'total_reads', 'identity_version']);
  });

  it('keeps Stream header help combining identity and status details', () => {
    const field = RECEIVER_HELP.streams.fields.stream_identity;

    expect(field.summary).toMatch(/forwarder.*reader/i);
    expect(field.summary).toMatch(/status|dot/i);
    expect(field.detailHtml).toContain('The dot next to each stream');
    expect(field.detailHtml).toContain('Reader down');
  });

  it('covers receiver Race Director configuration', () => {
    expect(RECEIVER_HELP).toHaveProperty('rd_import');
    expectFields(RECEIVER_HELP.rd_import, [
      'rd_import_enabled',
      'rd_import_dir',
      'rd_import_interval',
    ]);

    expect(RECEIVER_HELP).toHaveProperty('dbf_output');
    expectFields(RECEIVER_HELP.dbf_output, ['dbf_enabled', 'dbf_flush_interval', 'clear_dbf']);
  });

  it('covers receiver admin actions', () => {
    expectFields(RECEIVER_ADMIN_HELP.cursor_reset, [
      'stream_cursor',
      'reset_cursor',
      'reset_all_cursors',
    ]);
    expectFields(RECEIVER_ADMIN_HELP.epoch_overrides, [
      'epoch_override',
      'reset_epoch_override',
      'reset_all_epoch_overrides',
    ]);
    expectFields(RECEIVER_ADMIN_HELP.purge_subscriptions, ['purge_all_subscriptions']);
    expectFields(RECEIVER_ADMIN_HELP.reset_profile, ['reset_profile_action']);
    expect(RECEIVER_ADMIN_HELP).toHaveProperty('clear_data');
    expectFields(RECEIVER_ADMIN_HELP.clear_data, ['clear_data_action']);
    expectFields(RECEIVER_ADMIN_HELP.factory_reset, ['factory_reset_action']);
  });
});
