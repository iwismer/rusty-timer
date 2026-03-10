export type FieldHelp = {
  label: string;
  summary: string;
  /** Contains trusted HTML (rendered via {@html}). Must only come from static help data files — never user input. */
  detailHtml: string;
  default?: string;
  range?: string;
  recommended?: string;
};

export type SectionHelp = {
  title: string;
  overview: string;
  fields: Record<string, FieldHelp>;
  /** Contains trusted HTML (rendered via {@html}). Must only come from static help data files — never user input. */
  tips?: string[];
  seeAlso?: { sectionKey: string; label: string }[];
};

export type HelpContext = Record<string, SectionHelp>;
export type HelpContextName = "forwarder" | "receiver" | "receiver-admin" | "server";

export type HelpSearchResult = {
  context: HelpContextName;
  sectionKey: string;
  section: SectionHelp;
  matchedFields: Array<{ fieldKey: string; field: FieldHelp }>;
  matchedTips: string[];
};
