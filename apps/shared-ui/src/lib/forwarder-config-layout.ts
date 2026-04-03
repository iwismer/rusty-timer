export type ForwarderConfigSection =
  | 'general'
  | 'server'
  | 'auth'
  | 'journal'
  | 'uplink'
  | 'status_http'
  | 'readers'
  | 'ups';

export function getForwarderConfigSectionRows(): ForwarderConfigSection[][] {
  return [
    ['general', 'server'],
    ['auth', 'journal'],
    ['uplink', 'status_http'],
    ['readers', 'ups'],
  ];
}

export type FieldType = 'text' | 'number' | 'toggle';

export interface SectionField {
  key: string;
  label: string;
  type: FieldType;
  placeholder?: string;
  min?: number;
  max?: number;
}

export interface SectionLayout {
  key: ForwarderConfigSection;
  label: string;
  fields: SectionField[];
}

export const upsSectionLayout: SectionLayout = {
  key: 'ups',
  label: 'UPS (PiSugar)',
  fields: [
    { key: 'enabled', label: 'Enabled', type: 'toggle' },
    {
      key: 'daemon_addr',
      label: 'Daemon Address',
      type: 'text',
      placeholder: '127.0.0.1:8423',
    },
    {
      key: 'poll_interval_secs',
      label: 'Poll Interval (seconds)',
      type: 'number',
      min: 1,
      max: 60,
    },
    {
      key: 'upstream_heartbeat_secs',
      label: 'Heartbeat Interval (seconds)',
      type: 'number',
      min: 10,
      max: 300,
    },
  ],
};
