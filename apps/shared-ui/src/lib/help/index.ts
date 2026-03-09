import type { HelpContext, HelpContextName, SectionHelp, FieldHelp } from "./help-types";
import { FORWARDER_HELP } from "./forwarder-help";
import { RECEIVER_HELP } from "./receiver-help";
import { RECEIVER_ADMIN_HELP } from "./receiver-admin-help";

const CONTEXTS: Record<HelpContextName, HelpContext> = {
  forwarder: FORWARDER_HELP,
  receiver: RECEIVER_HELP,
  "receiver-admin": RECEIVER_ADMIN_HELP,
};

export function getSection(context: HelpContextName, sectionKey: string): SectionHelp | undefined {
  return CONTEXTS[context]?.[sectionKey];
}

export function getField(context: HelpContextName, sectionKey: string, fieldKey: string): FieldHelp | undefined {
  return CONTEXTS[context]?.[sectionKey]?.fields[fieldKey];
}

/** Search all help content across all contexts. Returns matches grouped by context+section. */
export function searchHelp(query: string): Array<{
  context: HelpContextName;
  sectionKey: string;
  section: SectionHelp;
  matchedFields: Array<{ fieldKey: string; field: FieldHelp }>;
  matchedTips: string[];
}> {
  if (!query.trim()) return [];
  const q = query.toLowerCase();
  const results: ReturnType<typeof searchHelp> = [];

  for (const [contextName, context] of Object.entries(CONTEXTS) as [HelpContextName, HelpContext][]) {
    for (const [sectionKey, section] of Object.entries(context)) {
      const matchedFields = Object.entries(section.fields)
        .filter(([, f]) =>
          [f.label, f.summary, f.detail, f.default, f.range, f.recommended]
            .some(text => text?.toLowerCase().includes(q))
        )
        .map(([fieldKey, field]) => ({ fieldKey, field }));

      const matchedTips = (section.tips ?? []).filter(t => t.toLowerCase().includes(q));

      const sectionMatches =
        section.title.toLowerCase().includes(q) ||
        section.overview.toLowerCase().includes(q);

      if (matchedFields.length > 0 || matchedTips.length > 0 || sectionMatches) {
        results.push({
          context: contextName,
          sectionKey,
          section,
          matchedFields: matchedFields.length > 0 ? matchedFields : Object.entries(section.fields).map(([k, f]) => ({ fieldKey: k, field: f })),
          matchedTips,
        });
      }
    }
  }
  return results;
}

export type { HelpContext, HelpContextName, SectionHelp, FieldHelp } from "./help-types";
