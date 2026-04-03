import { describe, expect, it } from 'vitest';
import { getForwarderConfigSectionRows, upsSectionLayout } from './forwarder-config-layout';

describe('getForwarderConfigSectionRows', () => {
  it('returns option-2 section groupings', () => {
    expect(getForwarderConfigSectionRows()).toEqual([
      ['general', 'server'],
      ['auth', 'journal'],
      ['uplink', 'status_http'],
      ['readers', 'ups'],
    ]);
  });
});

describe('upsSectionLayout', () => {
  it('has the expected key and label', () => {
    expect(upsSectionLayout.key).toBe('ups');
    expect(upsSectionLayout.label).toBe('UPS (PiSugar)');
  });

  it('defines four fields', () => {
    expect(upsSectionLayout.fields).toHaveLength(4);
    const keys = upsSectionLayout.fields.map((f) => f.key);
    expect(keys).toEqual([
      'enabled',
      'daemon_addr',
      'poll_interval_secs',
      'upstream_heartbeat_secs',
    ]);
  });
});
