export type FieldHelp = {
  label: string;
  summary: string;
  detail: string;
  default?: string;
  range?: string;
  recommended?: string;
};

export type SectionHelp = {
  title: string;
  overview: string;
  fields: Record<string, FieldHelp>;
  tips?: string[];
  seeAlso?: { sectionKey: string; label: string }[];
};

export type HelpContext = Record<string, SectionHelp>;
export type HelpContextName = "forwarder" | "receiver" | "receiver-admin";
