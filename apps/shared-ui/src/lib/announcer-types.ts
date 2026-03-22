/** Minimal stream info needed by the announcer config form. */
export interface AnnouncerStreamEntry {
  stream_id: string;
  forwarder_id: string;
  reader_ip: string;
  display_alias: string | null;
}

export interface AnnouncerConfig {
  enabled: boolean;
  enabled_until: string | null;
  selected_stream_ids: string[];
  /** Server-side list size cap. Valid range: 1..500. */
  max_list_size: number;
  updated_at: string;
  public_enabled: boolean;
}

export interface AnnouncerConfigUpdate {
  enabled: boolean;
  selected_stream_ids: string[];
  max_list_size: number;
}

export interface AnnouncerConfigApi {
  getStreams(): Promise<AnnouncerStreamEntry[]>;
  getConfig(): Promise<AnnouncerConfig>;
  saveConfig(update: AnnouncerConfigUpdate): Promise<AnnouncerConfig>;
  reset(): Promise<void>;
}
