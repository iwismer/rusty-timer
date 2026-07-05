import type { HelpContext, HelpContextName, SectionHelp, FieldHelp, HelpSearchResult } from "./help-types";
import { FORWARDER_HELP } from "./forwarder-help";
import { RECEIVER_HELP } from "./receiver-help";
import { RECEIVER_ADMIN_HELP } from "./receiver-admin-help";
import { SERVER_HELP } from "./server-help";
import { fieldMatchesQuery } from "./field-match";

const CONTEXTS: Record<HelpContextName, HelpContext> = {
  forwarder: FORWARDER_HELP,
  receiver: RECEIVER_HELP,
  "receiver-admin": RECEIVER_ADMIN_HELP,
  server: SERVER_HELP,
};

/** Look up a help section by context and section key. Returns undefined if not found. */
export function getSection(context: HelpContextName, sectionKey: string): SectionHelp | undefined {
  if (!(context in CONTEXTS)) {
    console.warn(`[help] Unknown context "${context}". Valid: ${Object.keys(CONTEXTS).join(", ")}`);
    return undefined;
  }
  return CONTEXTS[context][sectionKey];
}

/** Look up a field's help content by context, section key, and field key. Returns undefined if not found. */
export function getField(context: HelpContextName, sectionKey: string, fieldKey: string): FieldHelp | undefined {
  if (!(context in CONTEXTS)) {
    console.warn(`[help] Unknown context "${context}". Valid: ${Object.keys(CONTEXTS).join(", ")}`);
    return undefined;
  }
  return CONTEXTS[context][sectionKey]?.fields[fieldKey];
}

/** Search all help content across all contexts. Returns matches grouped by context+section.
 *  When no individual fields match (e.g., only the section title, overview, or tips matched), all fields from that section are included. */
export function searchHelp(query: string): HelpSearchResult[] {
  if (!query.trim()) return [];
  const q = query.toLowerCase();
  const results: HelpSearchResult[] = [];

  for (const [contextName, context] of Object.entries(CONTEXTS) as [HelpContextName, HelpContext][]) {
    for (const [sectionKey, section] of Object.entries(context)) {
      const matchedFields = Object.entries(section.fields)
        .filter(([, f]) => fieldMatchesQuery(f, query))
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

export type { HelpContext, HelpContextName, SectionHelp, FieldHelp, HelpSearchResult } from "./help-types";
